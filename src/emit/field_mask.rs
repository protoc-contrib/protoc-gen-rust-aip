//! Emits `MUTABLE_PATHS` — which fields of a resource an update may write —
//! and `implied_update_mask`, the mask AIP-134 implies when a client omits one.
//!
//! Read off `google.api.field_behavior` — everything not `OUTPUT_ONLY`,
//! `IDENTIFIER` or `IMMUTABLE`. The three are one question, not three: the
//! server owns it, it selects the target rather than being part of it, or it
//! was settable once, on create.
//!
//! # This does not validate a mask
//!
//! Checking that `update_mask.paths` names only these is protovalidate's, via
//! `(buf.validate.field).field_mask.in`, which the protovalidate-buffa plugin
//! implements as a first-class rule:
//!
//! ```proto
//! google.protobuf.FieldMask update_mask = 2 [
//!   (buf.validate.field).field_mask.in = ["display_name", "description"]
//! ];
//! ```
//!
//! That rule already matches subpaths, already reports as an ordinary violation
//! under rule id `field_mask.in`, and is already run by
//! `#[protovalidate_buffa::connect_impl]` along with every other rule on the
//! request. A check generated here would be a second implementation of it,
//! reported differently, that a handler had to remember to call.
//!
//! What protovalidate cannot do is *expand*. AIP-134 gives a mask two
//! shorthands, and a server has to turn each into a list of paths to write:
//!
//! - `*`, full replacement, is every writable field — `MUTABLE_PATHS`;
//! - an omitted mask is every writable field the client *populated* —
//!   `implied_update_mask()`.
//!
//! Deriving both from the schema is the point: a new writable field joins them
//! without anyone remembering to.
//!
//! Which leaves the `field_mask.in` annotation as a restatement of what
//! `field_behavior` already says. Keeping the two in step is a lint's job — the
//! same lint the `REQUIRED` case needs, for the same reason.
//!
//! See <https://google.aip.dev/134#field-masks>.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::{doc, short_name};
use crate::messages::{Field, Index, Kind, Message};
use crate::scan::{Registry, Resource};

/// Whether an update may write `field`.
fn writable(field: &Field) -> bool {
    !field.output_only && !field.identifier && !field.immutable
}

/// Emits `MUTABLE_PATHS` for every resource declared in `file`.
///
/// Per resource, not per update request: which of a resource's fields are
/// writable is a fact about the resource, and a schema may reach it through
/// more than one request or none yet.
#[must_use]
pub fn emit_file(file: &str, index: &Index, registry: &Registry) -> TokenStream {
    let blocks: Vec<TokenStream> = registry
        .by_file(file)
        .filter_map(|resource| emit_resource(file, index, resource))
        .collect();
    quote! { #( #blocks )* }
}

/// `MUTABLE_PATHS` for one resource, when it is declared on a message.
///
/// A file-scope `resource_definition` has no message to hang it off, and no
/// fields to read a behaviour from, so it gets nothing.
fn emit_resource(file: &str, index: &Index, resource: &Resource) -> Option<TokenStream> {
    let binding = resource.message.as_ref()?;
    let message = index
        .in_file(file)
        .find(|message| message.rust_path == binding.rust_path)?;

    let path: TokenStream = binding
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let literals: Vec<Literal> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .map(|field| Literal::string(&field.name))
        .collect();

    let short = short_name(&message.fqn);
    let paths_doc = doc(&format!(
        "The field paths an update may write on `{short}`.\n\n\
         Every field not annotated `OUTPUT_ONLY`, `IDENTIFIER` or \
         `IMMUTABLE`.\n\n\
         This is the *expansion* of the `*` mask, full replacement, which \
         AIP-134 reads as every writable field; an omitted mask is narrower — see \
         [`implied_update_mask`](Self::implied_update_mask). It is not the check \
         that a mask \
         names only these -- that is `(buf.validate.field).field_mask.in` on the \
         mask itself, which protovalidate runs along with every other rule on \
         the request.\n\n\
         Those two lists say the same thing and are kept in step by hand. A \
         schema that adds a writable field and forgets the annotation gets a \
         field this constant expands to and protovalidate then rejects.",
    ));

    let checks: Vec<TokenStream> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .map(|field| {
            let name = Literal::string(&field.name);
            let populated = populated(message, field);
            quote! {
                if #populated {
                    paths.push(#name);
                }
            }
        })
        .collect();
    let implied_doc = doc(&format!(
        "The update mask AIP-134 implies when a client omits one: every field \
         of [`MUTABLE_PATHS`](Self::MUTABLE_PATHS) this `{short}` populates, in \
         declaration order.\n\n\
         Populated as protobuf reads presence: a field with explicit presence — \
         `optional`, a message, a oneof member — when set, even to its zero \
         value; any other scalar when not its zero value; a repeated field or a \
         map when not empty.\n\n\
         Reads `self` and returns the paths; store them on the request's \
         `update_mask` when it is empty.\n\n\
         See <https://google.aip.dev/134#field-masks>.",
    ));

    Some(quote! {
        impl #path {
            #paths_doc
            pub const MUTABLE_PATHS: &'static [&'static str] = &[#( #literals ),*];

            #implied_doc
            #[must_use]
            pub fn implied_update_mask(&self) -> ::std::vec::Vec<&'static str> {
                let mut paths = ::std::vec::Vec::new();
                #( #checks )*
                paths
            }
        }
    })
}

/// An expression that is true when `field` is populated on `self`, the way
/// protobuf reads presence — protobuf-go's `Has`.
fn populated(message: &Message, field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if let Some(oneof) = &field.oneof {
        // Members share one `Option<Enum>`; this one is populated when it is
        // the member set.
        let oneof_ident = buffa_codegen::idents::make_field_ident(oneof);
        let enum_path: TokenStream = format!(
            "{}::{}",
            message.oneof_module,
            buffa_codegen::idents::to_upper_camel_case(oneof),
        )
        .parse()
        .expect("a oneof path built from proto identifiers is a valid Rust path");
        let variant = format_ident!(
            "{}",
            buffa_codegen::idents::to_upper_camel_case(&field.name)
        );
        return quote! {
            ::core::matches!(self.#oneof_ident, ::core::option::Option::Some(#enum_path::#variant(_)))
        };
    }
    if field.repeated || field.is_map {
        return quote! { !self.#ident.is_empty() };
    }
    if field.optional {
        return quote! { self.#ident.is_some() };
    }
    match field.kind {
        Kind::Message => quote! { self.#ident.as_option().is_some() },
        // By bit pattern, as protobuf-go does: `-0.0` is not the zero value.
        Kind::Double => quote! { self.#ident.to_bits() != 0 },
        // Any other scalar is populated when it is not its zero value.
        Kind::String | Kind::Bytes => quote! { !self.#ident.is_empty() },
        Kind::Bool => quote! { self.#ident },
        Kind::Integer => quote! { self.#ident != 0 },
        // `to_i32`, so an unknown value received on the wire counts too.
        Kind::Enum => quote! { self.#ident.to_i32() != 0 },
    }
}
