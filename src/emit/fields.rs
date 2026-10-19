//! Emits `aip::FieldResolver` for every message declared in a file: a field's name, or
//! names joined by `.` through singular message fields, resolved to its
//! `aip::FieldPath` with the field's number and type from the schema. It is what
//! lets a handler name a request field it wrote out --
//! `FieldPath::of::<UpdateRequest>("update_mask")` -- without copying the number
//! and type out of the proto, and have the name checked.
//!
//! A message gets an impl, empty of fields or not, as it does an
//! `aip::FieldBehavior`, so a consumer's code generic over `T: aip::FieldResolver`
//! needs none of its own.
//!
//! A field that holds a message generated in the same package resolves the rest
//! of a dotted name beneath it. One that holds a message from another package,
//! or a well-known type, ends the name there: `collection.display_name` walks
//! into `Collection`, `update_mask.paths` does not walk into `FieldMask`, and a
//! repeated field or a map ends it as well.

use std::collections::BTreeSet;

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::emit::behavior::field_path;
use crate::messages::{Field, Index, Kind, Message};

/// The `aip::FieldResolver` impl of each message declared in `file`, in declaration
/// order.
#[must_use]
pub fn emit_file(file: &str, index: &Index, generated: &BTreeSet<String>) -> TokenStream {
    let impls = index
        .in_file(file)
        .map(|message| emit_message(message, index, generated));
    quote! { #( #impls )* }
}

fn emit_message(message: &Message, index: &Index, generated: &BTreeSet<String>) -> TokenStream {
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");

    if message.fields.is_empty() {
        return quote! {
            impl ::aip::FieldResolver for #path {
                fn field_path(_name: &str) -> ::core::option::Option<::aip::FieldPath> {
                    ::core::option::Option::None
                }
            }
        };
    }

    let arms = message
        .fields
        .iter()
        .map(|field| arm(message, field, index, generated));
    quote! {
        impl ::aip::FieldResolver for #path {
            #[allow(
                clippy::too_many_lines,
                reason = "one arm per field, spelled out at generation time"
            )]
            fn field_path(name: &str) -> ::core::option::Option<::aip::FieldPath> {
                let (head, rest) = match name.split_once('.') {
                    ::core::option::Option::Some((head, rest)) => {
                        (head, ::core::option::Option::Some(rest))
                    }
                    ::core::option::Option::None => (name, ::core::option::Option::None),
                };
                match head {
                    #( #arms )*
                    _ => ::core::option::Option::None,
                }
            }
        }
    }
}

/// The arm for `field`: its path when the name ends there, and the rest of the
/// name resolved beneath the message it holds when that message is generated in
/// this package.
fn arm(
    message: &Message,
    field: &Field,
    index: &Index,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let name = Literal::string(&field.name);
    let path = field_path(field);
    if let Some(target) = beneath(message, field, index, generated) {
        quote! {
            #name => {
                let path = #path;
                match rest {
                    ::core::option::Option::None => ::core::option::Option::Some(path),
                    ::core::option::Option::Some(rest) => {
                        <#target as ::aip::FieldResolver>::field_path(rest)
                            .map(|beneath| path.join(beneath))
                    }
                }
            }
        }
    } else {
        quote! { #name => rest.is_none().then(|| #path), }
    }
}

/// The path of the message `field` holds, when a dotted name can walk into it:
/// a singular message field whose message is declared in this run and this
/// package, so its `FieldResolver` impl exists and is nameable here.
fn beneath(
    message: &Message,
    field: &Field,
    index: &Index,
    generated: &BTreeSet<String>,
) -> Option<TokenStream> {
    if field.kind != Kind::Message || field.repeated || field.is_map {
        return None;
    }
    let target = index.get(&field.type_name)?;
    if target.package != message.package || !generated.contains(&target.source_file) {
        return None;
    }
    Some(
        target
            .rust_path
            .parse()
            .expect("a message path built from proto identifiers is a valid Rust path"),
    )
}
