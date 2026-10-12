//! Emits a request's `aip::FieldBehavior` items: `validate_required`,
//! `imply_resource_name` and `imply_update_mask`, which apply a resource's own
//! items the way an AIP standard method asks. Their semantics are documented on
//! the trait, in aip-rs; aip-go's `ValidateRequired`, `ImplyResourceName` and
//! `ImplyUpdateMask` answer the same by reflection.
//!
//! Three shapes are recognized, on a service method's request and on the
//! entries of a batch one:
//!
//! - a create (AIP-133): a singular field whose type is a resource, and no
//!   `update_mask`. When it also has a `<resource field>_id` string, the server
//!   names the resource: `imply_resource_name`;
//! - an update (AIP-134): that resource field and a `google.protobuf.FieldMask
//!   update_mask` beside it. The schema, not the value, makes it an update, so
//!   an unset mask names nothing rather than reading as a create;
//! - a batch (AIP-233, AIP-235): a repeated `requests` whose entries are
//!   creates or updates, each handled as its own request under `requests[i]`.
//!
//! Anything else — a response holding a resource, a request of another shape —
//! gets none of these items, and the trait's defaults answer for it.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::messages::{Field, Index, Kind, Message};
use crate::scan::{Registry, Resource};

/// The request-level items for every create, update and batch request declared
/// in `file`, keyed by the message's fully-qualified name.
///
/// # Errors
///
/// If such a request's resource, or a batch's entry type, is declared in a
/// file not generated in this run: the emitted code calls its
/// `aip::FieldBehavior` impl, which only a run that generates its file emits.
pub fn emit_file(
    file: &str,
    index: &Index,
    registry: &Registry,
    generated: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<TokenStream>>> {
    let resources = resources(index, registry);
    let requests = requests(index);
    let mut items = BTreeMap::new();
    for message in index.in_file(file) {
        if !requests.contains(&message.fqn) {
            continue;
        }
        let Some(shape) = shape(index, &resources, message) else {
            continue;
        };
        check_generated(index, generated, message, &shape)?;
        items.insert(
            message.fqn.clone(),
            emit(index, &resources, message, &shape),
        );
    }
    Ok(items)
}

/// What a request is, by its fields.
enum Shape<'a> {
    /// A create, with the field holding its resource.
    Create { resource: &'a Field },
    /// An update, with its resource field and its `update_mask`.
    Update {
        resource: &'a Field,
        mask: &'a Field,
    },
    /// A batch, with its `requests` field, its entry type and whether its
    /// entries are updates.
    Batch {
        entries: &'a Field,
        entry: &'a Message,
        updates: bool,
    },
}

/// The messages declared as resources, by fully-qualified name, with their
/// declaration.
fn resources<'r>(index: &Index, registry: &'r Registry) -> BTreeMap<String, &'r Resource> {
    let bound: BTreeMap<(&str, &str), &Resource> = registry
        .resources
        .iter()
        .filter_map(|resource| {
            let binding = resource.message.as_ref()?;
            Some((
                (resource.package.as_str(), binding.rust_path.as_str()),
                resource,
            ))
        })
        .collect();
    index
        .all()
        .filter_map(|message| {
            let resource = bound.get(&(message.package.as_str(), message.rust_path.as_str()))?;
            Some((message.fqn.clone(), *resource))
        })
        .collect()
}

/// The messages a service method takes, and the entry types of their
/// `requests`: the only messages the request-level items are emitted on.
fn requests(index: &Index) -> BTreeSet<String> {
    let mut requests: BTreeSet<String> = index
        .methods
        .iter()
        .map(|method| method.input.clone())
        .collect();
    let entries: Vec<String> = requests
        .iter()
        .filter_map(|input| index.get(input))
        .filter_map(entries_field)
        .map(|field| field.type_name.clone())
        .collect();
    requests.extend(entries);
    requests
}

/// The batch `requests` field of `message`: repeated, of a message type.
fn entries_field(message: &Message) -> Option<&Field> {
    message.fields.iter().find(|field| {
        field.name == "requests" && field.repeated && !field.is_map && field.kind == Kind::Message
    })
}

/// Which standard method `message` is a request for, if any.
fn shape<'a>(
    index: &'a Index,
    resources: &BTreeMap<String, &Resource>,
    message: &'a Message,
) -> Option<Shape<'a>> {
    if let Some(entries) = entries_field(message) {
        let entry = index.get(&entries.type_name)?;
        return match single(resources, entry)? {
            Shape::Create { .. } => Some(Shape::Batch {
                entries,
                entry,
                updates: false,
            }),
            Shape::Update { .. } => Some(Shape::Batch {
                entries,
                entry,
                updates: true,
            }),
            Shape::Batch { .. } => None,
        };
    }
    single(resources, message)
}

/// Whether `message` is a create or an update request, by its fields.
fn single<'a>(resources: &BTreeMap<String, &Resource>, message: &'a Message) -> Option<Shape<'a>> {
    let resource = message.fields.iter().find(|field| {
        field.kind == Kind::Message
            && !field.repeated
            && !field.is_map
            && field.oneof.is_none()
            && resources.contains_key(&field.type_name)
    })?;
    let mask = message.fields.iter().find(|field| {
        field.name == "update_mask"
            && field.kind == Kind::Message
            && !field.repeated
            && field.type_name == ".google.protobuf.FieldMask"
    });
    Some(match mask {
        Some(mask) => Shape::Update { resource, mask },
        None => Shape::Create { resource },
    })
}

/// Fails if the emitted code would call the `aip::FieldBehavior` impl of a
/// message this run does not generate: the request's resource, or a batch's
/// entry type.
fn check_generated(
    index: &Index,
    generated: &BTreeSet<String>,
    request: &Message,
    shape: &Shape<'_>,
) -> Result<()> {
    let called = match shape {
        Shape::Create { resource } | Shape::Update { resource, .. } => resource,
        Shape::Batch { entries, .. } => entries,
    };
    let Some(target) = index.get(&called.type_name) else {
        return Ok(());
    };
    if generated.contains(&target.source_file) {
        return Ok(());
    }
    bail!(
        "{}.{}: the request-level field-behavior items call {}'s aip::FieldBehavior \
         impl, which is generated with {}; generate that file in the same run",
        request.fqn.trim_start_matches('.'),
        called.name,
        target.fqn.trim_start_matches('.'),
        target.source_file,
    )
}

/// The items for one request.
fn emit(
    index: &Index,
    resources: &BTreeMap<String, &Resource>,
    request: &Message,
    shape: &Shape<'_>,
) -> Vec<TokenStream> {
    match shape {
        Shape::Create { resource } => {
            let mut items = vec![validate(resource, &quote! { ::core::option::Option::None })];
            items.extend(name(index, resources, request, resource));
            items
        }
        Shape::Update { resource, mask } => {
            let ident = buffa_codegen::idents::make_field_ident(&mask.name);
            let paths = quote! {
                ::core::option::Option::Some(
                    self.#ident
                        .as_option()
                        .map_or(&[] as &[::std::string::String], |mask| mask.paths.as_slice()),
                )
            };
            vec![validate(resource, &paths), imply(resource, mask)]
        }
        Shape::Batch {
            entries,
            entry,
            updates,
        } => {
            let ident = buffa_codegen::idents::make_field_ident(&entries.name);
            let name_literal = Literal::string(&entries.name);
            let mut items = vec![quote! {
                fn validate_required(&self) -> ::core::result::Result<(), ::aip::RequiredError> {
                    ::aip::RequiredError::check(
                        self.#ident
                            .iter()
                            .enumerate()
                            .filter_map(|(index, request)| {
                                ::aip::FieldBehavior::validate_required(request)
                                    .err()
                                    .map(|error| error.under(::aip::FieldPath::index(#name_literal, index)))
                            })
                            .flat_map(|error| error.paths)
                            .collect(),
                    )
                }
            }];
            if *updates {
                items.push(quote! {
                    fn imply_update_mask(&mut self) {
                        for request in &mut self.#ident {
                            ::aip::FieldBehavior::imply_update_mask(request);
                        }
                    }
                });
            } else if let Some(Shape::Create { resource }) = single(resources, entry)
                && name(index, resources, entry, resource).is_some()
            {
                items.push(quote! {
                    fn imply_resource_name(
                        &mut self,
                        new_id: &mut dyn ::core::ops::FnMut() -> ::std::string::String,
                    ) {
                        for request in &mut self.#ident {
                            ::aip::FieldBehavior::imply_resource_name(request, new_id);
                        }
                    }
                });
            }
            items
        }
    }
}

/// `validate_required` for a create or update: the resource's
/// `validate_required_fields` with `mask`, each path under the resource field.
fn validate(resource: &Field, mask: &TokenStream) -> TokenStream {
    let ident = buffa_codegen::idents::make_field_ident(&resource.name);
    let name = Literal::string(&resource.name);
    quote! {
        fn validate_required(&self) -> ::core::result::Result<(), ::aip::RequiredError> {
            self.#ident.as_option().map_or(::core::result::Result::Ok(()), |resource| {
                ::aip::FieldBehavior::validate_required_fields(resource, #mask)
                    .map_err(|error| error.under(::aip::FieldPath::field(#name)))
            })
        }
    }
}

/// `imply_update_mask` for an update: the resource's `implied_update_mask`
/// into a mask that is unset or has no paths, when the resource is set.
fn imply(resource: &Field, mask: &Field) -> TokenStream {
    let resource = buffa_codegen::idents::make_field_ident(&resource.name);
    let mask = buffa_codegen::idents::make_field_ident(&mask.name);
    quote! {
        fn imply_update_mask(&mut self) {
            if self.#mask.as_option().is_some_and(|mask| !mask.paths.is_empty()) {
                return;
            }
            if let ::core::option::Option::Some(resource) = self.#resource.as_option() {
                let paths = ::aip::FieldBehavior::implied_update_mask(resource)
                    .into_iter()
                    .map(::std::borrow::ToOwned::to_owned)
                    .collect();
                self.#mask.get_or_insert_default().paths = paths;
            }
        }
    }
}

/// `imply_resource_name` for a create (AIP-133), or `None` when the request
/// leaves its resource for the handler to name.
///
/// Emitted when the request has a `<resource field>_id` string beside the
/// resource and the resource's first pattern ends in a variable: the id, minted
/// with `new_id` when omitted, fills that variable. A top-level pattern's
/// literal prefix comes before it (`books/{book}` names `books/<id>`), and a
/// nested one is put under the request's `parent` and the collection segment
/// before the variable (`shelves/{shelf}/books/{book}` names
/// `<parent>/books/<id>`), which needs a `parent` string on the request. The
/// request's `parent` is copied onto the resource's own `parent` string, when
/// both have one. The shape is decided here, from the pattern, so the emitted
/// code only formats.
fn name(
    index: &Index,
    resources: &BTreeMap<String, &Resource>,
    request: &Message,
    resource_field: &Field,
) -> Option<TokenStream> {
    let id_name = format!("{}_id", resource_field.name);
    let id = request
        .fields
        .iter()
        .find(|field| plain_string(field, &id_name))?;
    let resource = resources.get(&resource_field.type_name)?;
    let binding = resource.message.as_ref()?;
    let target = index.get(&resource_field.type_name)?;
    let pattern = resource.patterns.first()?;
    let (last, before) = pattern.segments.split_last()?;
    if !last.variable {
        return None;
    }

    let request_parent = request
        .fields
        .iter()
        .find(|field| plain_string(field, "parent"));
    let id_ident = buffa_codegen::idents::make_field_ident(&id.name);
    let value = if before.iter().any(|segment| segment.variable) {
        let collection = before.last().filter(|segment| !segment.variable)?;
        let parent = buffa_codegen::idents::make_field_ident(&request_parent?.name);
        let format = Literal::string(&format!("{{}}/{}/{{}}", collection.name));
        quote! { ::std::format!(#format, self.#parent, self.#id_ident) }
    } else {
        let prefix: Vec<&str> = before.iter().map(|segment| segment.name.as_str()).collect();
        let format = if prefix.is_empty() {
            "{}".to_owned()
        } else {
            format!("{}/{{}}", prefix.join("/"))
        };
        let format = Literal::string(&format);
        quote! { ::std::format!(#format, self.#id_ident) }
    };

    let name_ident = buffa_codegen::idents::make_field_ident(&binding.name_field);
    let assign_name = if binding.name_field_optional {
        quote! { resource.#name_ident = ::core::option::Option::Some(#value); }
    } else {
        quote! { resource.#name_ident = #value; }
    };
    let copy_parent = request_parent
        .zip(
            target
                .fields
                .iter()
                .find(|field| plain_string(field, "parent")),
        )
        .map(|(from, to)| {
            let from = buffa_codegen::idents::make_field_ident(&from.name);
            let to = buffa_codegen::idents::make_field_ident(&to.name);
            quote! { ::core::clone::Clone::clone_from(&mut resource.#to, &self.#from); }
        });
    let resource_ident = buffa_codegen::idents::make_field_ident(&resource_field.name);
    Some(quote! {
        fn imply_resource_name(
            &mut self,
            new_id: &mut dyn ::core::ops::FnMut() -> ::std::string::String,
        ) {
            if let ::core::option::Option::Some(resource) = self.#resource_ident.as_option_mut() {
                if self.#id_ident.is_empty() {
                    self.#id_ident = new_id();
                }
                #copy_parent
                #assign_name
            }
        }
    })
}

/// Whether `field` is a singular `string` without explicit presence named
/// `name`: what a create's id and `parent` are.
fn plain_string(field: &Field, name: &str) -> bool {
    field.name == name
        && field.kind == Kind::String
        && !field.repeated
        && !field.is_map
        && field.oneof.is_none()
        && !field.optional
}

#[cfg(test)]
mod tests {
    use buffa::ExtensionSet;
    use buffa_codegen::generated::{
        compiler::CodeGeneratorRequest,
        descriptor::{
            DescriptorProto, FieldDescriptorProto, FileDescriptorProto, MessageOptions,
            MethodDescriptorProto, ServiceDescriptorProto,
            field_descriptor_proto::{Label, Type},
        },
    };

    use super::*;
    use crate::annotations::google::api::{RESOURCE, ResourceDescriptor};

    /// `dep.proto` declares the resource `dep.Book`; `api.proto` declares
    /// `api.CreateBookRequest` holding one and a service taking it. Only the
    /// files in `generate` are generated.
    fn request(generate: &[&str]) -> CodeGeneratorRequest {
        let mut options = MessageOptions::default();
        options.set_extension(
            &RESOURCE,
            ResourceDescriptor {
                r#type: "example.com/Book".to_owned(),
                pattern: vec!["books/{book}".to_owned()],
                ..Default::default()
            },
        );
        let book = FileDescriptorProto {
            name: Some("dep.proto".to_owned()),
            package: Some("dep".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("Book".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("name".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_STRING),
                    label: Some(Label::LABEL_OPTIONAL),
                    ..Default::default()
                }],
                options: options.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let api = FileDescriptorProto {
            name: Some("api.proto".to_owned()),
            package: Some("api".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![DescriptorProto {
                name: Some("CreateBookRequest".to_owned()),
                field: vec![FieldDescriptorProto {
                    name: Some("book".to_owned()),
                    number: Some(1),
                    r#type: Some(Type::TYPE_MESSAGE),
                    type_name: Some(".dep.Book".to_owned()),
                    label: Some(Label::LABEL_OPTIONAL),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            service: vec![ServiceDescriptorProto {
                name: Some("Books".to_owned()),
                method: vec![MethodDescriptorProto {
                    name: Some("CreateBook".to_owned()),
                    input_type: Some(".api.CreateBookRequest".to_owned()),
                    output_type: Some(".dep.Book".to_owned()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        CodeGeneratorRequest {
            file_to_generate: generate.iter().map(|file| (*file).to_owned()).collect(),
            proto_file: vec![book, api],
            ..Default::default()
        }
    }

    fn emit_for(request: &CodeGeneratorRequest) -> Result<BTreeMap<String, Vec<TokenStream>>> {
        let index = crate::messages::gather(request);
        let registry = crate::scan::gather(request, false)?;
        let generated = request.file_to_generate.iter().cloned().collect();
        emit_file("api.proto", &index, &registry, &generated)
    }

    #[test]
    fn a_resource_from_a_file_not_generated_fails_generation() {
        let error = emit_for(&request(&["api.proto"])).expect_err("an impl the run does not emit");
        assert_eq!(
            error.to_string(),
            "api.CreateBookRequest.book: the request-level field-behavior items call dep.Book's \
             aip::FieldBehavior impl, which is generated with dep.proto; generate that file in \
             the same run"
        );
    }

    #[test]
    fn a_create_request_without_an_id_gets_only_the_check() {
        let items = emit_for(&request(&["api.proto", "dep.proto"])).expect("both files generated");
        let create = items
            .get(".api.CreateBookRequest")
            .expect("the create request gets items");
        assert_eq!(create.len(), 1);
        assert!(create[0].to_string().contains("validate_required"));
    }

    #[test]
    fn a_create_request_with_an_id_also_names_its_resource() {
        let mut request = request(&["api.proto", "dep.proto"]);
        request.proto_file[1].message_type[0]
            .field
            .push(FieldDescriptorProto {
                name: Some("book_id".to_owned()),
                number: Some(2),
                r#type: Some(Type::TYPE_STRING),
                label: Some(Label::LABEL_OPTIONAL),
                ..Default::default()
            });
        let items = emit_for(&request).expect("both files generated");
        let create = items
            .get(".api.CreateBookRequest")
            .expect("the create request gets items");
        assert_eq!(create.len(), 2);
        let name = create[1].to_string();
        assert!(name.contains("imply_resource_name"));
        assert!(name.contains("\"books/{}\""));
    }
}
