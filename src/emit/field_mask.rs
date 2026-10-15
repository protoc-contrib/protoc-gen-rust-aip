//! Emits a resource's `aip::Resource` impl: `implied_field_mask`, the mask
//! AIP-134 implies when an update omits one. Its semantics are documented on
//! the trait, in aip-rs. The resource's name is the message's own `parse_name`,
//! in `resource.rs`, not a part of this trait.
//!
//! The implied mask is read off `google.api.field_behavior`: every field an
//! update may write — not `OUTPUT_ONLY`, `IDENTIFIER` or `IMMUTABLE` — that the
//! value sets. The three exclusions are one question, not three: the server
//! owns it, it selects the target rather than being part of it, or it was
//! settable once, on create.
//!
//! # This does not validate a mask
//!
//! Checking that `update_mask.paths` names only writable fields is
//! protovalidate's, via `(buf.validate.field).field_mask.in`, which reports as
//! an ordinary violation along with every other rule on the request. A check
//! generated here would be a second implementation of it. What protovalidate
//! cannot do is *imply* a mask a client omitted, from the fields it set.
//!
//! See <https://google.aip.dev/134#field-masks>.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::messages::{Field, Index, Kind, Message};
use crate::scan::{Registry, Resource};

/// Whether an update may write `field`.
fn writable(field: &Field) -> bool {
    !field.output_only && !field.identifier && !field.immutable
}

/// The `impl ::aip::Resource` for each resource declared on a message in
/// `file`, in declaration order.
#[must_use]
pub fn emit_file(file: &str, index: &Index, registry: &Registry) -> TokenStream {
    let impls = registry
        .by_file(file)
        .filter_map(|resource| emit_resource(file, index, resource));
    quote! { #( #impls )* }
}

fn emit_resource(file: &str, index: &Index, resource: &Resource) -> Option<TokenStream> {
    let binding = resource.message.as_ref()?;
    let message = index
        .in_file(file)
        .find(|message| message.rust_path == binding.rust_path)?;
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let implied: Vec<TokenStream> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .map(|field| {
            let literal = Literal::string(&field.name);
            let push = quote! { paths.push(::std::borrow::ToOwned::to_owned(#literal)); };
            if field.required {
                return push;
            }
            let populated = populated(message, field);
            quote! {
                if #populated {
                    #push
                }
            }
        })
        .collect();
    let mutable = if implied.is_empty() {
        quote! { let paths = ::std::vec::Vec::new(); }
    } else {
        quote! { let mut paths = ::std::vec::Vec::new(); }
    };
    Some(quote! {
        impl ::aip::Resource for #path {
            fn implied_field_mask(&self) -> ::buffa_types::FieldMask {
                #mutable
                #( #implied )*
                ::buffa_types::FieldMask {
                    paths,
                    ..::core::default::Default::default()
                }
            }
        }
    })
}

/// The test for `field` being populated on `self`, as protobuf reads presence:
/// a field with explicit presence when set, even to its zero value; any other
/// scalar when not its zero value; a repeated field or a map when not empty.
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
