//! Emits the AIP-203 `OUTPUT_ONLY` clearing walk.
//!
//! Generated rather than reflective, and not as an optimisation: buffa 0.9
//! exposes only `reflect(&self)`, and marks `reflect_mut` as designed but
//! deferred, so nothing can mutate a message reflectively in any mode. The
//! read-only halves of AIP-203 stay in the runtime, where Go keeps all of it.
//!
//! Revisit the whole split if a buffa release lands `reflect_mut`.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::messages::{Field, Index, Message};

/// The messages a walk is emitted for.
///
/// A message needs one when it has an `OUTPUT_ONLY` field, or when any message
/// it can reach has one — clearing is recursive, so an untouched message that
/// merely *holds* a dirty one still has to be walked.
pub struct Walks {
    needed: BTreeSet<String>,
}

impl Walks {
    /// Whether a walk is emitted for the message with this fully-qualified
    /// name.
    #[must_use]
    pub fn contains(&self, fqn: &str) -> bool {
        self.needed.contains(fqn)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.needed.is_empty()
    }
}

/// Works out which messages need a walk.
///
/// `generated` is the set of files being generated. A message outside it gets
/// no walk, so nothing may recurse into one: the call would name a method that
/// was never emitted. Reachability is therefore computed over generated
/// messages only, which keeps the answer and the emitted code consistent.
#[must_use]
pub fn plan(index: &Index, generated: &BTreeSet<String>) -> Walks {
    let candidates: Vec<&Message> = generated
        .iter()
        .flat_map(|file| index.in_file(file))
        .collect();

    let mut needed: BTreeSet<String> = candidates
        .iter()
        .filter(|message| message.fields.iter().any(|field| field.output_only))
        .map(|message| message.fqn.clone())
        .collect();

    // A message holding a dirty message is itself dirty, so the answer has to
    // settle rather than be read off in one pass. Bounded by the number of
    // messages, since each round either adds one or stops.
    loop {
        let mut added = false;
        for message in &candidates {
            if needed.contains(&message.fqn) {
                continue;
            }
            if message
                .fields
                .iter()
                .filter_map(|field| reachable(field, index, generated))
                .any(|target| needed.contains(&target))
            {
                needed.insert(message.fqn.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    Walks { needed }
}

/// The message a field can carry a walk into, if any.
///
/// A field that is itself `OUTPUT_ONLY` is not one: it is about to be cleared
/// wholesale, so there is nothing beneath it worth visiting.
fn reachable(field: &Field, index: &Index, generated: &BTreeSet<String>) -> Option<String> {
    if field.output_only {
        return None;
    }
    let target = if field.is_map {
        field.map_value.clone()?
    } else if field.kind == crate::messages::Kind::Message {
        field.type_name.clone()
    } else {
        return None;
    };
    let message = index.get(&target)?;
    generated.contains(&message.source_file).then_some(target)
}

/// Emits the walks for the messages declared in one file.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    walks: &Walks,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let impls: Vec<TokenStream> = index
        .in_file(file)
        .filter(|message| walks.contains(&message.fqn))
        .map(|message| emit_message(message, index, walks, generated))
        .collect();
    quote! { #( #impls )* }
}

fn emit_message(
    message: &Message,
    index: &Index,
    walks: &Walks,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");

    // A oneof is one field on the struct, so its members are handled together
    // rather than one at a time.
    let mut oneofs: BTreeMap<&str, Vec<&Field>> = BTreeMap::new();
    let mut statements = Vec::new();
    for field in &message.fields {
        if let Some(oneof) = &field.oneof {
            oneofs.entry(oneof).or_default().push(field);
            continue;
        }
        if field.output_only {
            statements.push(clear(field));
        } else if let Some(target) = reachable(field, index, generated)
            && walks.contains(&target)
        {
            statements.push(descend(field));
        }
    }
    for (oneof, fields) in oneofs {
        statements.push(clear_oneof(
            message, oneof, &fields, index, walks, generated,
        ));
    }

    let cleared: Vec<&str> = message
        .fields
        .iter()
        .filter(|field| field.output_only)
        .map(|field| field.name.as_str())
        .collect();
    let summary = if cleared.is_empty() {
        "It has no `OUTPUT_ONLY` fields of its own; the walk is emitted because \
         a message beneath it does."
            .to_owned()
    } else {
        format!("Clears: `{}`.", cleared.join("`, `"))
    };
    let doc = super::doc(&format!(
        "Clears every field annotated `OUTPUT_ONLY`, at any depth.\n\n\
         {summary}\n\n\
         Call it on an inbound request to drop server-owned values a client \
         should not be able to set, rather than rejecting the request.\n\n\
         See <https://google.aip.dev/203>.",
    ));

    quote! {
        impl #path {
            #doc
            pub fn clear_output_only(&mut self) {
                #( #statements )*
            }
        }
    }
}

/// Clearing a field is assigning its default: for a proto3 field the default
/// *is* the unset value, and it is spelled the same way whatever the field
/// holds — `String`, `Vec`, `MessageField`, `EnumValue`, `Option`.
fn clear(field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    quote! { self.#ident = ::core::default::Default::default(); }
}

/// Recurses into whatever messages a field carries.
fn descend(field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if field.is_map {
        quote! {
            for value in self.#ident.values_mut() {
                value.clear_output_only();
            }
        }
    } else if field.repeated {
        quote! {
            for item in &mut self.#ident {
                item.clear_output_only();
            }
        }
    } else if field.proto3_optional {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_mut() {
                value.clear_output_only();
            }
        }
    } else {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_option_mut() {
                value.clear_output_only();
            }
        }
    }
}

/// Handles a real `oneof`, whose members share one `Option<Enum>` field.
///
/// An `OUTPUT_ONLY` member cannot be cleared on its own — there is no field to
/// assign to — so the oneof is cleared entirely when that member is the one
/// set. Any other member that carries a message is descended into instead.
fn clear_oneof(
    message: &Message,
    oneof: &str,
    fields: &[&Field],
    index: &Index,
    walks: &Walks,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(oneof);
    let enum_path: TokenStream = format!(
        "{}::{}",
        message.oneof_module,
        buffa_codegen::idents::to_upper_camel_case(oneof),
    )
    .parse()
    .expect("a oneof path built from proto identifiers is a valid Rust path");

    let clears: Vec<TokenStream> = fields
        .iter()
        .filter(|field| field.output_only)
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! { ::core::option::Option::Some(#enum_path::#variant(_)) }
        })
        .collect();

    let descents: Vec<TokenStream> = fields
        .iter()
        .filter(|field| {
            reachable(field, index, generated).is_some_and(|target| walks.contains(&target))
        })
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! {
                if let ::core::option::Option::Some(#enum_path::#variant(value)) =
                    &mut self.#ident
                {
                    value.clear_output_only();
                }
            }
        })
        .collect();

    // Tested and cleared as two statements rather than one `match`, because a
    // `match &mut self.<oneof>` holds the borrow that clearing it needs.
    let clear = if clears.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            if ::core::matches!(self.#ident, #( #clears )|*) {
                self.#ident = ::core::option::Option::None;
            }
        }
    };

    quote! {
        #clear
        #( #descents )*
    }
}

/// The enum variant buffa emits for a oneof member: the field name in
/// `UpperCamelCase`.
fn variant_ident(field: &str) -> proc_macro2::Ident {
    format_ident!("{}", buffa_codegen::idents::to_upper_camel_case(field))
}
