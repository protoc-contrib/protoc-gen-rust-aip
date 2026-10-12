//! Emits the AIP-203 clearing walks as `aip::FieldBehavior` items:
//! `clear_output_only`, which drops what a client may not set from a request,
//! and `clear_input_only`, which drops what a server may not return from a
//! response. One walk, run for either behavior.
//!
//! Generated rather than reflective, and not as an optimisation: buffa 0.9
//! exposes only `reflect(&self)`, and marks `reflect_mut` as designed but
//! deferred, so nothing can mutate a message reflectively in any mode. The
//! read-only halves of AIP-203 stay in the runtime, where Go keeps all of it.
//!
//! Revisit the whole split if a buffa release lands `reflect_mut`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::messages::{Field, Index, Message};

/// The field behavior a walk clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clearing {
    /// `OUTPUT_ONLY`: server-owned, cleared from an inbound request.
    OutputOnly,
    /// `INPUT_ONLY`: accepted on input, cleared from an outbound response.
    InputOnly,
}

impl Clearing {
    /// Whether `field` carries this behavior.
    fn marks(self, field: &Field) -> bool {
        match self {
            Self::OutputOnly => field.output_only,
            Self::InputOnly => field.input_only,
        }
    }

    /// The annotation value, as the schema spells it.
    fn annotation(self) -> &'static str {
        match self {
            Self::OutputOnly => "OUTPUT_ONLY",
            Self::InputOnly => "INPUT_ONLY",
        }
    }

    /// The name of the emitted method.
    fn method(self) -> &'static str {
        match self {
            Self::OutputOnly => "clear_output_only",
            Self::InputOnly => "clear_input_only",
        }
    }
}

/// The messages a walk is emitted for.
///
/// A message needs one when it has a field carrying the behavior, or when any
/// message it can reach has one — clearing is recursive, so an untouched
/// message that merely *holds* a dirty one still has to be walked.
pub struct Walks {
    clearing: Clearing,
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

/// Works out which messages need the walk that clears `clearing`.
///
/// `generated` is the set of files being generated. A message outside it gets
/// no walk, so nothing may recurse into one: the call would name a method that
/// was never emitted. Reachability is therefore computed over generated
/// messages only, which keeps the answer and the emitted code consistent.
///
/// # Errors
///
/// If a generated message holds one from a file *not* being generated that has
/// fields carrying the behavior beneath it. The walk cannot reach them: for
/// `OUTPUT_ONLY` a client could write them -- and an implied update mask, which
/// names the holding field as one path, would then store them; for
/// `INPUT_ONLY` a response would return them. Generate that file too.
pub fn plan(index: &Index, generated: &BTreeSet<String>, clearing: Clearing) -> Result<Walks> {
    check_unreachable(index, generated, clearing)?;
    let candidates: Vec<&Message> = generated
        .iter()
        .flat_map(|file| index.in_file(file))
        .collect();

    let mut needed: BTreeSet<String> = candidates
        .iter()
        .filter(|message| message.fields.iter().any(|field| clearing.marks(field)))
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
                .filter_map(|field| reachable(field, index, generated, clearing))
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

    Ok(Walks { clearing, needed })
}

/// Fails if a generated message holds a message the walk cannot reach that has
/// fields carrying the behavior beneath it. See [`plan`].
fn check_unreachable(
    index: &Index,
    generated: &BTreeSet<String>,
    clearing: Clearing,
) -> Result<()> {
    // Which messages, in any file, have such a field at any depth --
    // settled the same way `plan` settles its set, but over every file.
    let all: Vec<&Message> = index.all().collect();
    let mut dirty: BTreeSet<String> = all
        .iter()
        .filter(|message| message.fields.iter().any(|field| clearing.marks(field)))
        .map(|message| message.fqn.clone())
        .collect();
    loop {
        let mut added = false;
        for message in &all {
            if dirty.contains(&message.fqn) {
                continue;
            }
            if message
                .fields
                .iter()
                .filter_map(|field| carried(field, clearing))
                .any(|target| dirty.contains(&target))
            {
                dirty.insert(message.fqn.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    for message in generated.iter().flat_map(|file| index.in_file(file)) {
        for field in &message.fields {
            let Some(target) = carried(field, clearing) else {
                continue;
            };
            let Some(held) = index.get(&target) else {
                continue;
            };
            if !generated.contains(&held.source_file) && dirty.contains(&target) {
                bail!(
                    "{}.{}: holds {}, which has {} fields beneath it, but {} is not \
                     being generated, so {} cannot reach them; generate {} too",
                    message.fqn.trim_start_matches('.'),
                    field.name,
                    target.trim_start_matches('.'),
                    clearing.annotation(),
                    held.source_file,
                    clearing.method(),
                    held.source_file,
                );
            }
        }
    }
    Ok(())
}

/// The message a field carries, if any, whichever file it is in. Unlike
/// [`reachable`], which only answers for messages the walk can enter.
fn carried(field: &Field, clearing: Clearing) -> Option<String> {
    if clearing.marks(field) {
        return None;
    }
    if field.is_map {
        field.map_value.clone()
    } else if field.kind == crate::messages::Kind::Message {
        Some(field.type_name.clone())
    } else {
        None
    }
}

/// The message a field can carry a walk into, if any.
///
/// A field that itself carries the behavior is not one: it is about to be
/// cleared wholesale, so there is nothing beneath it worth visiting.
fn reachable(
    field: &Field,
    index: &Index,
    generated: &BTreeSet<String>,
    clearing: Clearing,
) -> Option<String> {
    if clearing.marks(field) {
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

/// Emits the walk for each message declared in one file that needs one, as a
/// `FieldBehavior` item keyed by the message's fully-qualified name, for
/// [`render`](super::render) to gather into the message's one impl.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    walks: &Walks,
    generated: &BTreeSet<String>,
) -> BTreeMap<String, TokenStream> {
    index
        .in_file(file)
        .filter(|message| walks.contains(&message.fqn))
        .map(|message| {
            (
                message.fqn.clone(),
                emit_message(message, index, walks, generated),
            )
        })
        .collect()
}

fn emit_message(
    message: &Message,
    index: &Index,
    walks: &Walks,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let clearing = walks.clearing;
    let method = format_ident!("{}", clearing.method());

    // A oneof is one field on the struct, so its members are handled together
    // rather than one at a time.
    let mut oneofs: BTreeMap<&str, Vec<&Field>> = BTreeMap::new();
    let mut statements = Vec::new();
    for field in &message.fields {
        if let Some(oneof) = &field.oneof {
            oneofs.entry(oneof).or_default().push(field);
            continue;
        }
        if clearing.marks(field) {
            statements.push(clear(field));
        } else if let Some(target) = reachable(field, index, generated, clearing)
            && walks.contains(&target)
        {
            statements.push(descend(field, &method));
        }
    }
    for (oneof, fields) in oneofs {
        statements.push(clear_oneof(
            message, oneof, &fields, index, walks, generated,
        ));
    }

    // `Default::default()` rather than a named type per field: that is what
    // lets one emitter clear a `String`, a `Vec`, a `MessageField`, an
    // `EnumValue` and an `Option` without this plugin duplicating buffa's
    // proto-to-Rust type mapping and going stale when it changes.
    //
    // A descent calls the walk on a nested message by method syntax, which
    // auto-derefs whatever buffa holds it in -- a `Box` for a oneof member --
    // where a fully-qualified call would have to name that pointer. The trait
    // needs no import for it: this is a method of the trait being implemented.
    quote! {
        #[allow(
            clippy::default_trait_access,
            reason = "the concrete type of each field is buffa's to decide, not this plugin's"
        )]
        fn #method(&mut self) {
            #( #statements )*
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
fn descend(field: &Field, method: &proc_macro2::Ident) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if field.is_map {
        quote! {
            for value in self.#ident.values_mut() {
                value.#method();
            }
        }
    } else if field.repeated {
        quote! {
            for item in &mut self.#ident {
                item.#method();
            }
        }
    } else if field.optional {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_mut() {
                value.#method();
            }
        }
    } else {
        quote! {
            if let ::core::option::Option::Some(value) = self.#ident.as_option_mut() {
                value.#method();
            }
        }
    }
}

/// Handles a real `oneof`, whose members share one `Option<Enum>` field.
///
/// A member carrying the behavior cannot be cleared on its own — there is no field to
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

    let clearing = walks.clearing;
    let method = format_ident!("{}", clearing.method());
    let clears: Vec<TokenStream> = fields
        .iter()
        .filter(|field| clearing.marks(field))
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! { ::core::option::Option::Some(#enum_path::#variant(_)) }
        })
        .collect();

    let descents: Vec<TokenStream> = fields
        .iter()
        .filter(|field| {
            reachable(field, index, generated, clearing)
                .is_some_and(|target| walks.contains(&target))
        })
        .map(|field| {
            let variant = variant_ident(&field.name);
            quote! {
                if let ::core::option::Option::Some(#enum_path::#variant(value)) =
                    &mut self.#ident
                {
                    value.#method();
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

#[cfg(test)]
mod tests {
    use buffa::ExtensionSet;
    use buffa_codegen::generated::{
        compiler::CodeGeneratorRequest,
        descriptor::{
            DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto,
            field_descriptor_proto::{Label, Type},
        },
    };

    use super::*;
    use crate::annotations::google::api::{FIELD_BEHAVIOR, FieldBehavior};

    /// `dep.proto` declares `dep.Audit` with a `create_time` carrying
    /// `behavior`; `api.proto` declares `api.Thing` holding one. Only
    /// `api.proto` is generated.
    fn request(behavior: Option<FieldBehavior>) -> CodeGeneratorRequest {
        let mut options = FieldOptions::default();
        if let Some(behavior) = behavior {
            options.set_extension(&FIELD_BEHAVIOR, vec![behavior as i32]);
        }
        let audit = FileDescriptorProto {
            name: Some("dep.proto".to_owned()),
            package: Some("dep".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Audit".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("create_time".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_STRING),
                    label: Some(Label::LABEL_OPTIONAL),
                    options: options.into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let thing = FileDescriptorProto {
            name: Some("api.proto".to_owned()),
            package: Some("api".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Thing".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("audit".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_MESSAGE),
                    type_name: Some(".dep.Audit".to_owned()),
                    label: Some(Label::LABEL_OPTIONAL),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        CodeGeneratorRequest {
            file_to_generate: vec!["api.proto".to_owned()],
            proto_file: vec![audit, thing],
            ..Default::default()
        }
    }

    fn plan_for(request: &CodeGeneratorRequest, clearing: Clearing) -> Result<Walks> {
        let index = crate::messages::gather(request);
        let generated = request.file_to_generate.iter().cloned().collect();
        plan(&index, &generated, clearing)
    }

    #[test]
    fn output_only_fields_in_a_dependency_fail_generation() {
        let error = plan_for(
            &request(Some(FieldBehavior::OUTPUT_ONLY)),
            Clearing::OutputOnly,
        )
        .err()
        .expect("an unreachable OUTPUT_ONLY field");
        assert_eq!(
            error.to_string(),
            "api.Thing.audit: holds dep.Audit, which has OUTPUT_ONLY fields beneath it, but \
             dep.proto is not being generated, so clear_output_only cannot reach them; \
             generate dep.proto too"
        );
    }

    #[test]
    fn input_only_fields_in_a_dependency_fail_generation_too() {
        let error = plan_for(
            &request(Some(FieldBehavior::INPUT_ONLY)),
            Clearing::InputOnly,
        )
        .err()
        .expect("an unreachable INPUT_ONLY field");
        assert_eq!(
            error.to_string(),
            "api.Thing.audit: holds dep.Audit, which has INPUT_ONLY fields beneath it, but \
             dep.proto is not being generated, so clear_input_only cannot reach them; \
             generate dep.proto too"
        );
    }

    #[test]
    fn each_walk_reaches_only_its_own_behavior() {
        let output_only = request(Some(FieldBehavior::OUTPUT_ONLY));
        assert!(plan_for(&output_only, Clearing::InputOnly).is_ok());
        let input_only = request(Some(FieldBehavior::INPUT_ONLY));
        assert!(plan_for(&input_only, Clearing::OutputOnly).is_ok());
    }

    #[test]
    fn a_dependency_with_nothing_output_only_is_fine() {
        assert!(plan_for(&request(None), Clearing::OutputOnly).is_ok());
    }
}
