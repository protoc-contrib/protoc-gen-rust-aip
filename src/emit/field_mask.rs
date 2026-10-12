//! Emits a resource's `aip::FieldBehavior` items: `MUTABLE_PATHS` — which
//! fields an update may write — and `implied_update_mask`, the mask AIP-134
//! implies when a client omits one, along with the two AIP-203 checks an update
//! or create needs: `validate_immutable` and `validate_required_fields`. Their
//! semantics are documented on the trait, in aip-rs.
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
//! `field_behavior` already says. Keeping the two in step is a lint's job.
//!
//! # `REQUIRED` is checked here, not by protovalidate
//!
//! A resource message is shared by its create and update requests, and a
//! `REQUIRED` field means something different in each: a create must set it,
//! an update may omit it unless its mask names it. A rule on the resource
//! cannot tell which request it is in, so protovalidate can only enforce it
//! through a CEL rule restated on every request. `validate_required_fields`
//! reads the annotation instead, with the mask the request carries.
//!
//! See <https://google.aip.dev/134#field-masks>.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

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
///
/// # Errors
///
/// If a `REQUIRED` field `validate_required_fields` has to check sits in a oneof
/// of a message from another package, whose enum the emitted code cannot name.
pub fn emit_file(
    file: &str,
    index: &Index,
    registry: &Registry,
) -> Result<BTreeMap<String, Vec<TokenStream>>> {
    let mut items = BTreeMap::new();
    for resource in registry.by_file(file) {
        if let Some((fqn, resource_items)) = emit_resource(file, index, resource)? {
            items.insert(fqn, resource_items);
        }
    }
    Ok(items)
}

/// The `FieldBehavior` items for one resource, when it is declared on a
/// message, keyed by the message's fully-qualified name -- only those that
/// differ from the trait's "nothing to do" defaults.
///
/// A file-scope `resource_definition` has no message to hang them off, and no
/// fields to read a behaviour from, so it gets nothing.
fn emit_resource(
    file: &str,
    index: &Index,
    resource: &Resource,
) -> Result<Option<(String, Vec<TokenStream>)>> {
    let Some(binding) = resource.message.as_ref() else {
        return Ok(None);
    };
    let Some(message) = index
        .in_file(file)
        .find(|message| message.rust_path == binding.rust_path)
    else {
        return Ok(None);
    };

    let mut items = Vec::new();

    let literals: Vec<Literal> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .map(|field| Literal::string(&field.name))
        .collect();
    if !literals.is_empty() {
        items.push(quote! {
            const MUTABLE_PATHS: &'static [&'static str] = &[#( #literals ),*];
        });
    }

    let checks: Vec<TokenStream> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .map(|field| {
            let name = Literal::string(&field.name);
            // A required field is set on every message that parsed -- what
            // protobuf-go's `Has` reports -- and has no zero value to test.
            if field.required {
                return quote! { paths.push(#name); };
            }
            let populated = populated(&quote! { self }, message, field);
            quote! {
                if #populated {
                    paths.push(#name);
                }
            }
        })
        .collect();
    if !checks.is_empty() {
        let body = collect_into("paths", &checks);
        items.push(quote! {
            fn implied_update_mask(&self) -> ::std::vec::Vec<&'static str> {
                #body
            }
        });
    }

    let immutable = immutable_checks(message);
    if !immutable.is_empty() {
        items.push(quote! {
            fn validate_immutable(
                &self,
                existing: &Self,
            ) -> ::core::result::Result<(), ::aip::ImmutableError> {
                let mut changed: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
                #( #immutable )*
                ::aip::ImmutableError::check(
                    changed.into_iter().map(::aip::FieldPath::from).collect(),
                )
            }
        });
    }

    if let Some(required) = validate_required_fields_fn(index, message)? {
        items.push(required);
    }

    Ok(Some((message.fqn.clone(), items)))
}

/// What [`required_checks`] found to emit for one message.
#[derive(Default)]
struct Required {
    statements: Vec<TokenStream>,
    /// Whether any statement descends into a nested message, which needs the
    /// `reaches` closure as well as `covered`.
    descends: bool,
}

/// `validate_required_fields`, or `None` when nothing beneath `resource` is
/// `REQUIRED` and the trait's default already answers.
fn validate_required_fields_fn(index: &Index, resource: &Message) -> Result<Option<TokenStream>> {
    let mut seen = vec![resource.fqn.clone()];
    let required = required_checks(
        index,
        &resource.package,
        resource,
        &quote! { self },
        "",
        &mut seen,
    )?;
    if required.statements.is_empty() {
        return Ok(None);
    }

    // A path is covered when the mask names it or a message above it -- the
    // update writes it then -- and reached when it is covered or the mask
    // names something beneath it, which is when a nested message is worth
    // entering.
    let reaches = if required.descends {
        quote! {
            let reaches = |path: &str| {
                covered(path)
                    || mask.is_some_and(|paths| {
                        paths.iter().any(|named| {
                            named.strip_prefix(path).is_some_and(|rest| rest.starts_with('.'))
                        })
                    })
            };
        }
    } else {
        TokenStream::new()
    };
    let statements = required.statements;
    Ok(Some(quote! {
        #[allow(
            clippy::collapsible_if,
            reason = "collapsing a test and a binding takes a let chain, which needs edition 2024"
        )]
        #[allow(
            clippy::too_many_lines,
            reason = "one statement per REQUIRED path, spelled out at generation time"
        )]
        fn validate_required_fields(
            &self,
            mask: ::core::option::Option<&[::std::string::String]>,
        ) -> ::core::result::Result<(), ::aip::RequiredError> {
            let covered = |path: &str| {
                mask.is_none_or(|paths| {
                    paths.iter().any(|named| {
                        named == path
                            || path
                                .strip_prefix(named.as_str())
                                .is_some_and(|rest| rest.starts_with('.'))
                    })
                })
            };
            #reaches
            let mut missing: ::std::vec::Vec<&'static str> = ::std::vec::Vec::new();
            #( #statements )*
            ::aip::RequiredError::check(
                missing.into_iter().map(::aip::FieldPath::from).collect(),
            )
        }
    }))
}

/// The statements checking the `REQUIRED` fields of `message`, read through
/// `receiver`, whose paths start with `prefix`, and descending into every set
/// singular message that has some beneath it.
///
/// Paths are spelled out at generation time, so each is a `&'static str`. That
/// is why a message type already on the way down (`seen`) is not entered
/// again: a recursive type would otherwise never stop expanding.
fn required_checks(
    index: &Index,
    package: &str,
    message: &Message,
    receiver: &TokenStream,
    prefix: &str,
    seen: &mut Vec<String>,
) -> Result<Required> {
    let mut required = Required::default();
    for field in &message.fields {
        let path = format!("{prefix}{}", field.name);
        let literal = Literal::string(&path);

        // proto2 `required` is set on every message that parsed.
        if field.must_set && !field.required {
            check_oneof_reachable(package, message, field)?;
            let unset = unset(receiver, message, field);
            required.statements.push(quote! {
                if covered(#literal) && #unset {
                    missing.push(#literal);
                }
            });
        }

        if field.kind != Kind::Message || field.repeated || field.is_map {
            continue;
        }
        let Some(nested) = index.get(&field.type_name) else {
            continue;
        };
        if seen.contains(&nested.fqn) {
            continue;
        }
        seen.push(nested.fqn.clone());
        let inner = required_checks(
            index,
            package,
            nested,
            &quote! { value },
            &format!("{path}."),
            seen,
        )?;
        seen.pop();
        if inner.statements.is_empty() {
            continue;
        }

        check_oneof_reachable(package, message, field)?;
        let ident = buffa_codegen::idents::make_field_ident(&field.name);
        let statements = inner.statements;
        // Unset, the message is read as empty when the mask names only paths
        // beneath it: the update writes those, so it would clear them. Named
        // whole -- or on a create -- an unset message is not entered, since a
        // nested message's annotations apply only when it is set.
        seen.push(nested.fqn.clone());
        let empty = unset_checks(index, nested, &format!("{path}."), seen);
        seen.pop();
        let otherwise = if empty.is_empty() {
            TokenStream::new()
        } else {
            quote! {
                else if !covered(#literal) {
                    #( #empty )*
                }
            }
        };
        let entered = if let Some(oneof) = &field.oneof {
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
            quote! {
                if let ::core::option::Option::Some(#enum_path::#variant(value)) = &#receiver.#oneof_ident {
                    #( #statements )*
                } #otherwise
            }
        } else {
            quote! {
                if let ::core::option::Option::Some(value) = #receiver.#ident.as_option() {
                    #( #statements )*
                } #otherwise
            }
        };
        required.statements.push(quote! {
            if reaches(#literal) {
                #entered
            }
        });
        required.descends = true;
    }
    Ok(required)
}

/// The statements checking the `REQUIRED` fields of an *unset* `message`,
/// whose paths start with `prefix`: the [`required_checks`] an empty message
/// would get, decided here rather than by reading one. In an empty message
/// every `REQUIRED` field is unset -- the proto2 label included, which only
/// a message that parsed is sure to carry -- so each is missing wherever the
/// mask covers it, and a message beneath is again entered only when the mask
/// names paths beneath it alone. Nothing is read, so a oneof member needs no
/// enum path, and no message type has to be named to build an empty one.
fn unset_checks(
    index: &Index,
    message: &Message,
    prefix: &str,
    seen: &mut Vec<String>,
) -> Vec<TokenStream> {
    let mut statements = Vec::new();
    for field in &message.fields {
        let path = format!("{prefix}{}", field.name);
        let literal = Literal::string(&path);
        if field.must_set {
            statements.push(quote! {
                if covered(#literal) {
                    missing.push(#literal);
                }
            });
        }
        if field.kind != Kind::Message || field.repeated || field.is_map {
            continue;
        }
        let Some(nested) = index.get(&field.type_name) else {
            continue;
        };
        if seen.contains(&nested.fqn) {
            continue;
        }
        seen.push(nested.fqn.clone());
        let inner = unset_checks(index, nested, &format!("{path}."), seen);
        seen.pop();
        if !inner.is_empty() {
            statements.push(quote! {
                if !covered(#literal) && reaches(#literal) {
                    #( #inner )*
                }
            });
        }
    }
    statements
}

/// Fails if `field` is a oneof member of a message outside `package` that
/// `validate_required_fields` has to read: the emitted check names the oneof's
/// enum by its path within the package, which does not reach another
/// package's.
fn check_oneof_reachable(package: &str, message: &Message, field: &Field) -> Result<()> {
    if field.oneof.is_some() && message.package != package {
        bail!(
            "{}.{}: validate_required_fields for a resource in {package} has to read this oneof \
             member, but its enum belongs to {}, which the emitted code cannot name; drop \
             REQUIRED from it and from everything beneath it, or move the message into \
             {package}",
            message.fqn.trim_start_matches('.'),
            field.name,
            message.package,
        );
    }
    Ok(())
}

/// A body collecting `statements`' pushes into a vector named `name`.
fn collect_into(name: &str, statements: &[TokenStream]) -> TokenStream {
    let name = format_ident!("{name}");
    quote! {
        let mut #name = ::std::vec::Vec::new();
        #( #statements )*
        #name
    }
}

/// One statement per writable-once field of `message` -- `IMMUTABLE` and not
/// `OUTPUT_ONLY`, which clearing already drops -- pushing its name onto
/// `changed` when `self` sets it to something other than `existing` holds.
fn immutable_checks(message: &Message) -> Vec<TokenStream> {
    message
        .fields
        .iter()
        .filter(|field| field.immutable && !field.output_only)
        .map(|field| {
            let name = Literal::string(&field.name);
            let ident = buffa_codegen::idents::make_field_ident(&field.name);
            let set = if field.required {
                quote! { true }
            } else {
                populated(&quote! { self }, message, field)
            };
            // By bit pattern for a float, as `populated` reads one, so `-0.0`
            // and `NaN` compare the way protobuf-go's `proto.Equal` does not
            // need to be argued about.
            let differs = match (field.kind, field.optional) {
                // The service does not keep the order, so neither does the
                // comparison: equal when every element is matched by one of
                // the stored list's, each used once.
                _ if field.unordered_list && field.repeated && !field.is_map => quote! {
                    {
                        let (update, stored) = (&self.#ident, &existing.#ident);
                        let mut taken = ::std::vec![false; stored.len()];
                        update.len() != stored.len()
                            || !update.iter().all(|element| {
                                stored.iter().zip(taken.iter_mut()).any(|(candidate, taken)| {
                                    let matched = !*taken && candidate == element;
                                    *taken |= matched;
                                    matched
                                })
                            })
                    }
                },
                (Kind::Double, false) => {
                    quote! { self.#ident.to_bits() != existing.#ident.to_bits() }
                }
                (Kind::Double, true) => quote! {
                    self.#ident.map(|value| value.to_bits())
                        != existing.#ident.map(|value| value.to_bits())
                },
                _ if field.oneof.is_some() => {
                    let oneof = buffa_codegen::idents::make_field_ident(
                        field.oneof.as_deref().unwrap_or_default(),
                    );
                    quote! { self.#oneof != existing.#oneof }
                }
                _ => quote! { self.#ident != existing.#ident },
            };
            quote! {
                if #set && #differs {
                    changed.push(#name);
                }
            }
        })
        .collect()
}

/// An expression that is true when `field` is *not* populated on `receiver` --
/// [`populated`] negated, spelled so no consumer's clippy sees a `!` it could
/// simplify.
fn unset(receiver: &TokenStream, message: &Message, field: &Field) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    if field.oneof.is_some() {
        let populated = populated(receiver, message, field);
        return quote! { !#populated };
    }
    if field.repeated || field.is_map {
        return quote! { #receiver.#ident.is_empty() };
    }
    if field.optional {
        return quote! { #receiver.#ident.is_none() };
    }
    match field.kind {
        Kind::Message => quote! { #receiver.#ident.as_option().is_none() },
        Kind::Double => quote! { #receiver.#ident.to_bits() == 0 },
        Kind::String | Kind::Bytes => quote! { #receiver.#ident.is_empty() },
        Kind::Bool => quote! { !#receiver.#ident },
        Kind::Integer => quote! { #receiver.#ident == 0 },
        Kind::Enum => quote! { #receiver.#ident.to_i32() == 0 },
    }
}

/// An expression that is true when `field` is populated on `receiver`, the way
/// protobuf reads presence — protobuf-go's `Has`. `receiver` is `self` or a
/// binding to a nested `message`; a oneof member names `message`'s enum by its
/// path within the package, so a nested message must share the package.
fn populated(receiver: &TokenStream, message: &Message, field: &Field) -> TokenStream {
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
            ::core::matches!(#receiver.#oneof_ident, ::core::option::Option::Some(#enum_path::#variant(_)))
        };
    }
    if field.repeated || field.is_map {
        return quote! { !#receiver.#ident.is_empty() };
    }
    if field.optional {
        return quote! { #receiver.#ident.is_some() };
    }
    match field.kind {
        Kind::Message => quote! { #receiver.#ident.as_option().is_some() },
        // By bit pattern, as protobuf-go does: `-0.0` is not the zero value.
        Kind::Double => quote! { #receiver.#ident.to_bits() != 0 },
        // Any other scalar is populated when it is not its zero value.
        Kind::String | Kind::Bytes => quote! { !#receiver.#ident.is_empty() },
        Kind::Bool => quote! { #receiver.#ident },
        Kind::Integer => quote! { #receiver.#ident != 0 },
        // `to_i32`, so an unknown value received on the wire counts too.
        Kind::Enum => quote! { #receiver.#ident.to_i32() != 0 },
    }
}

#[cfg(test)]
mod tests {
    use buffa::ExtensionSet;
    use buffa_codegen::generated::{
        compiler::CodeGeneratorRequest,
        descriptor::{
            DescriptorProto, FieldDescriptorProto, FieldOptions, FileDescriptorProto,
            OneofDescriptorProto,
            field_descriptor_proto::{Label, Type},
        },
    };

    use super::*;
    use crate::annotations::google::api::{FIELD_BEHAVIOR, FieldBehavior};

    /// `dep.proto` declares `dep.Payment`, whose oneof `method` has a member
    /// `card`; `api.proto` declares `api.Order` holding one. `card` is
    /// `REQUIRED` when `required_member` is set.
    fn index(required_member: bool) -> Index {
        let mut options = FieldOptions::default();
        if required_member {
            options.set_extension(&FIELD_BEHAVIOR, vec![FieldBehavior::REQUIRED as i32]);
        }
        let payment = FileDescriptorProto {
            name: Some("dep.proto".to_owned()),
            package: Some("dep".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Payment".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("card".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_STRING),
                    label: Some(Label::LABEL_OPTIONAL),
                    oneof_index: Some(0),
                    options: options.into(),
                    ..Default::default()
                }],
                oneof_decl: vec![OneofDescriptorProto {
                    name: Some("method".to_owned()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let order = FileDescriptorProto {
            name: Some("api.proto".to_owned()),
            package: Some("api".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Order".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("payment".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_MESSAGE),
                    type_name: Some(".dep.Payment".to_owned()),
                    label: Some(Label::LABEL_OPTIONAL),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::messages::gather(&CodeGeneratorRequest {
            file_to_generate: vec!["api.proto".to_owned()],
            proto_file: vec![payment, order],
            ..Default::default()
        })
    }

    fn validate_required_fields_for(index: &Index) -> Result<Option<TokenStream>> {
        let order = index.get(".api.Order").expect("api.Order is indexed");
        validate_required_fields_fn(index, order)
    }

    #[test]
    fn a_required_oneof_member_in_another_package_fails_generation() {
        let error = validate_required_fields_for(&index(true))
            .expect_err("a oneof the emitted code cannot name");
        assert_eq!(
            error.to_string(),
            "dep.Payment.card: validate_required_fields for a resource in api has to read \
             this oneof member, but its enum belongs to dep, which the emitted code cannot name; drop \
             REQUIRED from it and from everything beneath it, or move the message into api"
        );
    }

    #[test]
    fn a_oneof_in_another_package_with_nothing_required_is_fine() {
        assert!(validate_required_fields_for(&index(false)).is_ok());
    }
}
