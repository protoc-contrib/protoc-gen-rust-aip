//! Emits the AIP-134 `update_mask` check, over `google.protobuf.FieldMask`.
//!
//! Two halves. On the *resource*, which fields an update may write, from
//! `google.api.field_behavior`: everything not `OUTPUT_ONLY`, `IDENTIFIER` or
//! `IMMUTABLE`. On the *update request*, a check of `update_mask.paths`
//! against it.
//!
//! Paths are matched by walking, not by enumeration. A mask may name a nested
//! path — `carrier.name` — and enumerating every such path is unbounded the
//! moment a schema has a message that can reach itself. Walking recurses on the
//! path the client sent, which is finite by construction, so a cyclic schema
//! costs nothing and needs no depth limit.
//!
//! See <https://google.aip.dev/134#field-masks>.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::{doc, impl_for, short_name, type_path_in};
use crate::messages::{Field, Index, Kind, Message};
use crate::scan::Registry;

/// The proto type of the field an update request carries its mask in.
const FIELD_MASK: &str = ".google.protobuf.FieldMask";

/// The field name AIP-134 gives that mask.
const UPDATE_MASK: &str = "update_mask";

/// Field number of `paths` on `google.protobuf.FieldMask`.
const PATHS_NUMBER: i32 = 1;

/// The rule id a rejected path is reported under.
///
/// Named the way a `buf.validate` CEL rule would be, since that is where this
/// rule would live if protovalidate-buffa's transpiler could compile it.
const RULE_ID: &str = "update_mask.mutable_paths";

/// What the pass worked out for one request.
pub struct Updates {
    /// Update request FQN to the FQN of the resource it updates.
    requests: BTreeMap<String, String>,
    /// Every message an `is_mutable_path` is emitted for: each updated
    /// resource, and every message reachable from one through a writable
    /// message-typed field.
    checked: BTreeSet<String>,
}

impl Updates {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

/// Whether an update may write `field`.
///
/// The three behaviours are read as one question rather than three, because
/// they answer one: `OUTPUT_ONLY` is the server's to set, `IDENTIFIER` selects
/// the target rather than being part of it, and `IMMUTABLE` was settable once,
/// on create. A field carrying any of them is not writable now.
fn writable(field: &Field) -> bool {
    !field.output_only && !field.identifier && !field.immutable
}

/// Finds the update requests, and the messages their masks can reach.
///
/// A request is an update request when it carries a
/// `google.protobuf.FieldMask` named `update_mask` and exactly one other
/// message field whose type is a declared resource. That resource is what the
/// mask's paths are relative to.
///
/// Structural, like the List detection, rather than matching the name
/// `Update<Resource>Request`: the shape is what the mask is checked against, so
/// a request that has the shape and not the name still works, and one with the
/// name and not the shape is not silently half-supported.
#[must_use]
pub fn plan(index: &Index, registry: &Registry, generated: &BTreeSet<String>) -> Updates {
    let mut requests = BTreeMap::new();
    let mut checked = BTreeSet::new();

    for file in generated {
        for message in index.in_file(file) {
            let Some(resource) = updated_resource(message, index, registry) else {
                continue;
            };
            requests.insert(message.fqn.clone(), resource.clone());
            checked.insert(resource);
        }
    }

    // A mask path may descend through a writable message field, so every
    // message one can reach needs a checker too. A fixpoint rather than a
    // recursion, so a schema whose messages can reach themselves terminates.
    loop {
        let mut added = false;
        for fqn in checked.clone() {
            let Some(message) = index.get(&fqn) else {
                continue;
            };
            for target in message
                .fields
                .iter()
                .filter_map(|field| nested(field, index, generated))
            {
                added |= checked.insert(target);
            }
        }
        if !added {
            break;
        }
    }

    Updates { requests, checked }
}

/// The resource an update request updates, if `message` is one.
fn updated_resource(message: &Message, index: &Index, registry: &Registry) -> Option<String> {
    let has_mask = message
        .fields
        .iter()
        .any(|field| field.name == UPDATE_MASK && field.type_name == FIELD_MASK);
    if !has_mask {
        return None;
    }

    let mut resources = message
        .fields
        .iter()
        .filter(|field| field.kind == Kind::Message && !field.repeated)
        .filter(|field| field.type_name != FIELD_MASK)
        .filter(|field| is_resource(&field.type_name, index, registry));

    // Exactly one, or the mask has no unambiguous subject and this is some
    // other message that happens to carry a field mask.
    let only = resources.next()?;
    resources.next().is_none().then(|| only.type_name.clone())
}

/// Whether a message type is a declared resource.
fn is_resource(type_name: &str, index: &Index, registry: &Registry) -> bool {
    let Some(message) = index.get(type_name) else {
        return false;
    };
    registry.resources.iter().any(|resource| {
        resource.message.as_ref().is_some_and(|binding| {
            binding.rust_path == message.rust_path && resource.package == message.package
        })
    })
}

/// The message a mask path can descend into through `field`.
///
/// Only a singular message field. A path cannot name an element of a repeated
/// or map field — AIP-134 masks address fields, not entries — so neither is a
/// way down.
fn nested(field: &Field, index: &Index, generated: &BTreeSet<String>) -> Option<String> {
    if !writable(field) || field.repeated || field.is_map || field.kind != Kind::Message {
        return None;
    }
    let message = index.get(&field.type_name)?;
    generated
        .contains(&message.source_file)
        .then(|| field.type_name.clone())
}

/// Emits both halves for the messages declared in one file.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    updates: &Updates,
    generated: &BTreeSet<String>,
    views: bool,
) -> TokenStream {
    let blocks: Vec<TokenStream> = index
        .in_file(file)
        .filter_map(|message| {
            let checker = updates
                .checked
                .contains(&message.fqn)
                .then(|| emit_checker(message, index, updates, generated));
            let request = updates
                .requests
                .get(&message.fqn)
                .map(|resource| emit_request(message, index, resource, views));
            (checker.is_some() || request.is_some()).then(|| quote! { #checker #request })
        })
        .collect();

    quote! { #( #blocks )* }
}

/// `MUTABLE_PATHS` and `is_mutable_path` on a message a mask addresses.
fn emit_checker(
    message: &Message,
    index: &Index,
    updates: &Updates,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");

    let writable: Vec<&Field> = message
        .fields
        .iter()
        .filter(|field| writable(field))
        .collect();
    let literals: Vec<Literal> = writable
        .iter()
        .map(|field| Literal::string(&field.name))
        .collect();

    let arms: Vec<TokenStream> = writable
        .iter()
        .map(|field| {
            let name = Literal::string(&field.name);
            if let Some(target) =
                nested(field, index, generated).filter(|target| updates.checked.contains(target))
            {
                // A writable message field: the path may stop here, replacing
                // the whole subtree, or carry on into it.
                let target: TokenStream = type_path_in(message, index, &target);
                quote! {
                    (#name, ::core::option::Option::None) => true,
                    (#name, ::core::option::Option::Some(rest)) => {
                        #target::is_mutable_path(rest)
                    }
                }
            } else {
                // A leaf, or a message this plugin did not generate a checker
                // for. Either way there is nothing below it that can be
                // checked, so a path that carries on is rejected rather than
                // waved through.
                quote! { (#name, ::core::option::Option::None) => true }
            }
        })
        .collect();

    let short = short_name(&message.fqn);
    let paths_doc = doc(&format!(
        "The top-level field paths an `update_mask` may name on `{short}`.\n\n\
         Every field not annotated `OUTPUT_ONLY`, `IDENTIFIER` or `IMMUTABLE`. \
         A mask may also name a path *beneath* one of these, which \
         [`is_mutable_path`](Self::is_mutable_path) is for; this constant is \
         the expansion of an absent or empty mask, which AIP-134 reads as \
         every writable field.",
    ));
    let check_doc = doc(&format!(
        "Whether `path` names a field an update may write on `{short}`, at any \
         depth.\n\n\
         A dotted path is walked segment by segment, so `a.b` is writable when \
         `a` is a writable message field and `b` is writable on its type. \
         Naming a message field on its own is writable too: replacing a subtree \
         is writing it.\n\n\
         Nothing is enumerated, so a schema whose messages can reach themselves \
         is fine — the recursion is on `path`, which is finite.",
    ));

    quote! {
        impl #path {
            #paths_doc
            pub const MUTABLE_PATHS: &'static [&'static str] = &[#( #literals ),*];

            #check_doc
            #[must_use]
            #[allow(
                clippy::match_like_matches_macro,
                clippy::match_same_arms,
                reason = "one arm per field: collapsing them would hide which fields are covered, \
                          and a schema with one writable leaf would read differently from one with ten"
            )]
            pub fn is_mutable_path(path: &str) -> bool {
                let (head, rest) = match path.split_once('.') {
                    ::core::option::Option::Some((head, rest)) => {
                        (head, ::core::option::Option::Some(rest))
                    }
                    ::core::option::Option::None => (path, ::core::option::Option::None),
                };
                match (head, rest) {
                    #( #arms, )*
                    _ => false,
                }
            }
        }
    }
}

/// `validate_update_mask` on the request carrying the mask.
///
/// Reports a `protovalidate_buffa::ValidationError` rather than an error of its
/// own, because that is what this is: a rule about a field, on a request whose
/// other rules are already protovalidate's. A bad mask then reaches the client
/// as an ordinary violation — same `invalid_argument`, same
/// `update_mask.paths[1]` field path — and a client cannot tell which rules were
/// transpiled from `buf.validate` and which were generated from
/// `google.api.field_behavior`.
///
/// The rule cannot be written in the schema instead. It needs to say "every path
/// is one of these", and protovalidate-buffa transpiles CEL ahead of time:
/// `this.paths.all(...)` is outside the subset it supports.
///
/// An inherent method, not a `Validate` impl. The protovalidate plugin already
/// owns `Validate` for this type, and a second impl of the same trait for the
/// same type is a coherence error; suppressing the generated one would mean
/// hand-maintaining the rules the schema does declare.
fn emit_request(message: &Message, index: &Index, resource: &str, views: bool) -> TokenStream {
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let mask = buffa_codegen::idents::make_field_ident(UPDATE_MASK);
    let mask_name = Literal::string(UPDATE_MASK);
    let mask_number = Literal::i32_suffixed(
        message
            .fields
            .iter()
            .find(|field| field.name == UPDATE_MASK)
            .map_or(0, |field| field.number),
    );
    let target: TokenStream = type_path_in(message, index, resource);

    let short = short_name(resource);
    let rule_id = Literal::string(RULE_ID);
    let method_doc = doc(&format!(
        "Checks that `update_mask` names only fields `{short}` allows \
         updating.\n\n\
         An unset mask names no paths and so cannot name a bad one; AIP-134 \
         reads it as every writable field, which is \
         [`MUTABLE_PATHS`]({short}::MUTABLE_PATHS). An empty one is the same.\n\n\
         Not run by `Validate::validate`, and so not by \
         `#[protovalidate_buffa::connect_impl]` either — the protovalidate \
         plugin owns that trait for this type. Call it alongside.\n\n\
         # Errors\n\n\
         A `ValidationError` carrying one violation per offending path, each \
         pointing at `{}[i]`, so a client naming two bad paths is told about \
         both and gets the same error shape every other rule produces.",
        format_args!("{UPDATE_MASK}.paths"),
    ));

    impl_for(
        &path,
        views,
        &quote! {
            #method_doc
            pub fn validate_update_mask(
                &self,
            ) -> ::core::result::Result<(), ::protovalidate_buffa::ValidationError> {
                let ::core::option::Option::Some(mask) = self.#mask.as_option() else {
                    return ::core::result::Result::Ok(());
                };
                let violations: ::std::vec::Vec<::protovalidate_buffa::Violation> = mask
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| !#target::is_mutable_path(path))
                    .map(|(index, path)| ::protovalidate_buffa::Violation {
                        field: ::protovalidate_buffa::FieldPath {
                            elements: ::std::vec![
                                ::protovalidate_buffa::FieldPathElement {
                                    field_number: ::core::option::Option::Some(#mask_number),
                                    field_name: ::core::option::Option::Some(
                                        ::std::borrow::Cow::Borrowed(#mask_name),
                                    ),
                                    field_type: ::core::option::Option::Some(
                                        ::protovalidate_buffa::FieldType::Message,
                                    ),
                                    key_type: ::core::option::Option::None,
                                    value_type: ::core::option::Option::None,
                                    subscript: ::core::option::Option::None,
                                },
                                ::protovalidate_buffa::FieldPathElement {
                                    field_number: ::core::option::Option::Some(#PATHS_NUMBER),
                                    field_name: ::core::option::Option::Some(
                                        ::std::borrow::Cow::Borrowed("paths"),
                                    ),
                                    field_type: ::core::option::Option::Some(
                                        ::protovalidate_buffa::FieldType::String,
                                    ),
                                    key_type: ::core::option::Option::None,
                                    value_type: ::core::option::Option::None,
                                    subscript: ::core::option::Option::Some(
                                        ::protovalidate_buffa::Subscript::Index(index as u64),
                                    ),
                                },
                            ],
                        },
                        // Empty: there is no `buf.validate` rule to point back
                        // at, which is the whole reason this is generated.
                        rule: ::core::default::Default::default(),
                        rule_id: ::std::borrow::Cow::Borrowed(#rule_id),
                        message: ::std::borrow::Cow::Owned(
                            ::std::format!("`{path}` is not an updatable field path"),
                        ),
                        for_key: false,
                    })
                    .collect();
                if violations.is_empty() {
                    return ::core::result::Result::Ok(());
                }
                ::core::result::Result::Err(::protovalidate_buffa::ValidationError {
                    violations,
                    compile_error: ::core::option::Option::None,
                    runtime_error: ::core::option::Option::None,
                })
            }
        },
    )
}
