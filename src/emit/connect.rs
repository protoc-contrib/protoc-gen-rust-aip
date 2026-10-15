//! Emits what aip-rs's `FieldBehaviorInterceptor` (its default `connect`
//! feature) is driven by: `aip::ResourceWrites` on every message a service
//! takes, and one `PROCEDURES` table per package naming each method's message
//! types. A resource needs nothing more: aip-rs makes every `aip::Resource` an
//! `aip::DynResource`.
//!
//! A request's shape is read off its fields, as aip-go recognizes it by
//! reflection:
//!
//! - a create (AIP-133): a singular field whose type is a resource, and no
//!   `update_mask`; its `<resource field>_id`, when it has one, is the id the
//!   interceptor fills in when omitted;
//! - an update (AIP-134): that resource field and a `google.protobuf.FieldMask
//!   update_mask` beside it. The schema, not the value, makes it an update;
//! - a batch (AIP-233, AIP-235): a repeated `requests` whose entries are
//!   creates or updates, each written under `requests[i]`.
//!
//! Any other request writes nothing, and gets an empty impl, so the table can
//! name every request alike.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::{Options, package_depth};
use crate::messages::{Field, Index, Kind, Message, Method};
use crate::scan::{Registry, Resource};

/// The `google.protobuf` messages buffa-types generates, which aip-rs gives
/// empty `aip::FieldBehavior` impls: a method may take or return one without
/// it being generated in this run.
const WELL_KNOWN: &[&str] = &[
    "Any",
    "Api",
    "BoolValue",
    "BytesValue",
    "DoubleValue",
    "Duration",
    "Empty",
    "Enum",
    "EnumValue",
    "Field",
    "FieldMask",
    "FloatValue",
    "Int32Value",
    "Int64Value",
    "ListValue",
    "Method",
    "Mixin",
    "Option",
    "SourceContext",
    "StringValue",
    "Struct",
    "Timestamp",
    "Type",
    "UInt32Value",
    "UInt64Value",
    "Value",
];

/// `aip::ResourceWrites` for every request declared in `file`.
///
/// # Errors
///
/// If a request's resource, or a batch's entry type, is declared in a file not
/// generated in this run: the emitted code needs its `aip::Resource` or
/// `aip::ResourceWrites` impl, which only a run that generates its file emits.
pub fn emit_file(
    file: &str,
    index: &Index,
    registry: &Registry,
    generated: &BTreeSet<String>,
) -> Result<TokenStream> {
    let resources = resources(index, registry);
    let requests = requests(index);
    let mut impls = Vec::new();
    for message in index.in_file(file) {
        if !requests.contains(&message.fqn) {
            continue;
        }
        let shape = shape(index, &resources, message);
        if let Some(shape) = &shape {
            check_generated(index, generated, message, shape)?;
        }
        impls.push(resource_writes(message, shape.as_ref()));
    }
    Ok(quote! { #( #impls )* })
}

/// The `PROCEDURES` table for `package`: one `aip::Procedure` per method its
/// generated files' services declare, in declaration order, or `None` when
/// they declare none.
///
/// A method of any cardinality is `Procedure::new`, the full policy on each
/// message, unless its request is a well-known type: that has nothing to fill
/// in or validate, and no `Validate` impl, so it is `Procedure::clearing`,
/// which clears only.
///
/// # Errors
///
/// If a method's message type is neither generated in this run nor a
/// well-known type, so has no `aip::FieldBehavior` impl to name; or if
/// `proto_module` is not a Rust path.
pub fn procedures(
    package: &str,
    index: &Index,
    generated: &BTreeSet<String>,
    options: &Options,
) -> Result<Option<TokenStream>> {
    let methods: Vec<&Method> = index
        .methods
        .iter()
        .filter(|method| method.package == package && generated.contains(&method.file))
        .collect();
    if methods.is_empty() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for method in methods {
        let path = Literal::string(&method.procedure());
        let input = message_type(package, index, generated, options, method, &method.input)?;
        let output = message_type(package, index, generated, options, method, &method.output)?;
        let constructor = if well_known(&method.input).is_some() {
            quote! { clearing }
        } else {
            quote! { new }
        };
        entries.push(quote! {
            ::aip::Procedure::#constructor::<#input, #output>(#path)
        });
    }
    let doc = super::doc(&format!(
        "Every procedure the services of `{package}` declare, with the message\n\
         types `aip::FieldBehaviorInterceptor` decodes for it: the full AIP\n\
         field-behavior policy on each message, unary or streamed, and clearing\n\
         only for a request that is a well-known type.\n\
         \n\
         ```ignore\n\
         service.with_interceptor(aip::FieldBehaviorInterceptor::new(PROCEDURES));\n\
         ```"
    ));
    Ok(Some(quote! {
        #doc
        pub const PROCEDURES: &[::aip::Procedure] = &[ #( #entries, )* ];
    }))
}

/// The Rust type of the message `fqn` that `method` takes or returns, from
/// the module emitted for `package`: a message generated in this run, or a
/// well-known type from buffa-types.
fn message_type(
    package: &str,
    index: &Index,
    generated: &BTreeSet<String>,
    options: &Options,
    method: &Method,
    fqn: &str,
) -> Result<TokenStream> {
    if let Some(message) = index
        .get(fqn)
        .filter(|message| generated.contains(&message.source_file))
    {
        return message_path(package, message, options);
    }
    if let Some(name) = well_known(fqn) {
        let name = format_ident!("{}", name);
        return Ok(quote! { ::buffa_types::google::protobuf::#name });
    }
    bail!(
        "{}: PROCEDURES needs {}'s aip::FieldBehavior impl, which is generated with its \
         file; generate that file in the same run",
        method.procedure().trim_start_matches('/'),
        fqn.trim_start_matches('.'),
    )
}

/// The name of the well-known type `fqn`, if it is one buffa-types generates.
fn well_known(fqn: &str) -> Option<&str> {
    fqn.strip_prefix(".google.protobuf.")
        .filter(|name| WELL_KNOWN.contains(name))
}

/// The path to `message` from the module emitted for `from_package`: its short
/// path within the same package; through `proto_module` from another when the
/// module tree is emitted, or up to the root and back down when the output is
/// included beside the message types.
fn message_path(from_package: &str, message: &Message, options: &Options) -> Result<TokenStream> {
    let path: TokenStream = message
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    if message.package == from_package {
        return Ok(path);
    }
    let downs: Vec<TokenStream> = message
        .package
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let segment = format_ident!("{}", buffa_codegen::idents::escape_mod_ident(segment));
            quote! { #segment }
        })
        .collect();
    if options.packaging {
        let root: syn::Path = syn::parse_str(&options.proto_module).with_context(|| {
            format!("proto_module {:?} is not a Rust path", options.proto_module)
        })?;
        return Ok(quote! { #root #( :: #downs )* :: #path });
    }
    let ups = std::iter::repeat_n(quote! { super }, package_depth(from_package));
    Ok(quote! { #( #ups :: )* #( #downs :: )* #path })
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
    /// A batch, with its `requests` field.
    Batch { entries: &'a Field },
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
/// `requests`: the messages `aip::ResourceWrites` is emitted on.
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
            Shape::Create { .. } | Shape::Update { .. } => Some(Shape::Batch { entries }),
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

/// Fails if the emitted code would need an impl this run does not emit: the
/// request's resource's `aip::Resource`, or a batch entry's
/// `aip::ResourceWrites`.
fn check_generated(
    index: &Index,
    generated: &BTreeSet<String>,
    request: &Message,
    shape: &Shape<'_>,
) -> Result<()> {
    let called = match shape {
        Shape::Create { resource } | Shape::Update { resource, .. } => resource,
        Shape::Batch { entries } => entries,
    };
    let Some(target) = index.get(&called.type_name) else {
        return Ok(());
    };
    if generated.contains(&target.source_file) {
        return Ok(());
    }
    bail!(
        "{}.{}: aip::ResourceWrites needs {}'s impls, which are generated with {}; \
         generate that file in the same run",
        request.fqn.trim_start_matches('.'),
        called.name,
        target.fqn.trim_start_matches('.'),
        target.source_file,
    )
}

/// `aip::ResourceWrites` for one request: the writes of its shape, or an empty
/// impl.
fn resource_writes(request: &Message, shape: Option<&Shape<'_>>) -> TokenStream {
    let path: TokenStream = request
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let Some(shape) = shape else {
        return quote! { impl ::aip::ResourceWrites for #path {} };
    };
    let body = match shape {
        Shape::Create { resource } => {
            let field = buffa_codegen::idents::make_field_ident(&resource.name);
            let step = step(resource);
            let id_name = format!("{}_id", resource.name);
            let id = request
                .fields
                .iter()
                .find(|field| plain_string(field, &id_name))
                .map_or_else(
                    || quote! { ::core::option::Option::None },
                    |id| {
                        let id = buffa_codegen::idents::make_field_ident(&id.name);
                        quote! { ::core::option::Option::Some(&mut self.#id) }
                    },
                );
            quote! {
                if let ::core::option::Option::Some(resource) = self.#field.as_option_mut() {
                    f(::aip::ResourceWrite {
                        path: ::aip::FieldPath::field(#step),
                        resource,
                        method: ::aip::WriteMethod::Create { id: #id },
                    });
                }
            }
        }
        Shape::Update { resource, mask } => {
            let field = buffa_codegen::idents::make_field_ident(&resource.name);
            let step = step(resource);
            let mask = buffa_codegen::idents::make_field_ident(&mask.name);
            quote! {
                if let ::core::option::Option::Some(resource) = self.#field.as_option_mut() {
                    f(::aip::ResourceWrite {
                        path: ::aip::FieldPath::field(#step),
                        resource,
                        method: ::aip::WriteMethod::Update {
                            mask: self.#mask.get_or_insert_default(),
                        },
                    });
                }
            }
        }
        Shape::Batch { entries } => {
            let field = buffa_codegen::idents::make_field_ident(&entries.name);
            let step = step(entries);
            quote! {
                for (index, request) in self.#field.iter_mut().enumerate() {
                    ::aip::ResourceWrites::for_each_write(request, &mut |write| {
                        f(write.prefixed(&::aip::FieldPath::index(#step, index)));
                    });
                }
            }
        }
    };
    quote! {
        impl ::aip::ResourceWrites for #path {
            fn for_each_write(&mut self, f: &mut dyn ::core::ops::FnMut(::aip::ResourceWrite<'_>)) {
                #body
            }
        }
    }
}

/// The name, number and type an `aip::FieldPath` step to the message field
/// `field` takes: `"book", 2, ::aip::FieldType::Message`.
fn step(field: &Field) -> TokenStream {
    let name = Literal::string(&field.name);
    let number = Literal::u32_unsuffixed(u32::try_from(field.number).unwrap_or_default());
    let ty = format_ident!("{}", field.field_type);
    quote! { #name, #number, ::aip::FieldType::#ty }
}

/// Whether `field` is a singular `string` without explicit presence named
/// `name`: what a create's id is.
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

    fn string_field(name: &str, number: i32) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.to_owned()),
            number: Some(number),
            r#type: Some(Type::TYPE_STRING),
            label: Some(Label::LABEL_OPTIONAL),
            ..Default::default()
        }
    }

    /// `dep.proto` declares the resource `dep.Book`; `api.proto` declares
    /// `api.CreateBookRequest` holding one, `api.GetBookRequest`, and a service
    /// taking both, whose `GetBook` returns `google.protobuf.Empty`. Only the
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
                field: vec![string_field("name", 1)],
                options: options.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let api = FileDescriptorProto {
            name: Some("api.proto".to_owned()),
            package: Some("api".to_owned()),
            syntax: Some("proto3".to_owned()),
            message_type: vec![
                DescriptorProto {
                    name: Some("CreateBookRequest".to_owned()),
                    field: vec![
                        FieldDescriptorProto {
                            name: Some("book".to_owned()),
                            number: Some(1),
                            r#type: Some(Type::TYPE_MESSAGE),
                            type_name: Some(".dep.Book".to_owned()),
                            label: Some(Label::LABEL_OPTIONAL),
                            ..Default::default()
                        },
                        string_field("book_id", 2),
                    ],
                    ..Default::default()
                },
                DescriptorProto {
                    name: Some("GetBookRequest".to_owned()),
                    field: vec![string_field("name", 1)],
                    ..Default::default()
                },
            ],
            service: vec![ServiceDescriptorProto {
                name: Some("Books".to_owned()),
                method: vec![
                    MethodDescriptorProto {
                        name: Some("CreateBook".to_owned()),
                        input_type: Some(".api.CreateBookRequest".to_owned()),
                        output_type: Some(".dep.Book".to_owned()),
                        ..Default::default()
                    },
                    MethodDescriptorProto {
                        name: Some("GetBook".to_owned()),
                        input_type: Some(".api.GetBookRequest".to_owned()),
                        output_type: Some(".google.protobuf.Empty".to_owned()),
                        ..Default::default()
                    },
                ],
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

    fn generated(request: &CodeGeneratorRequest) -> BTreeSet<String> {
        request.file_to_generate.iter().cloned().collect()
    }

    fn emit(request: &CodeGeneratorRequest, file: &str) -> Result<String> {
        let index = crate::messages::gather(request);
        let registry = crate::scan::gather(request, false)?;
        emit_file(file, &index, &registry, &generated(request)).map(|tokens| tokens.to_string())
    }

    #[test]
    fn a_resource_from_a_file_not_generated_fails_generation() {
        let error = emit(&request(&["api.proto"]), "api.proto").expect_err("an impl not emitted");
        assert_eq!(
            error.to_string(),
            "api.CreateBookRequest.book: aip::ResourceWrites needs dep.Book's impls, \
             which are generated with dep.proto; generate that file in the same run"
        );
    }

    #[test]
    fn a_create_writes_its_resource_and_any_other_request_writes_nothing() {
        let api = emit(&request(&["api.proto", "dep.proto"]), "api.proto").expect("emitted");
        assert!(api.contains("impl :: aip :: ResourceWrites for CreateBookRequest"));
        assert!(api.contains(
            "path : :: aip :: FieldPath :: field (\"book\" , 1 , :: aip :: FieldType :: Message)"
        ));
        assert!(api.contains(
            "WriteMethod :: Create { id : :: core :: option :: Option :: Some (& mut self . book_id) }"
        ));
        assert!(api.contains("impl :: aip :: ResourceWrites for GetBookRequest { }"));
    }

    #[test]
    fn a_resource_gets_no_connect_impl() {
        let dep = emit(&request(&["api.proto", "dep.proto"]), "dep.proto").expect("emitted");
        assert!(dep.is_empty(), "every aip::Resource is an aip::DynResource");
    }

    #[test]
    fn a_well_known_type_is_named_from_buffa_types() {
        let request = request(&["api.proto", "dep.proto"]);
        let index = crate::messages::gather(&request);
        let table = procedures("api", &index, &generated(&request), &Options::default())
            .expect("emitted")
            .expect("the package declares a service")
            .to_string();
        assert!(table.contains(
            "Procedure :: new :: < CreateBookRequest , crate :: proto :: dep :: Book > (\"/api.Books/CreateBook\")"
        ));
        assert!(table.contains(
            "Procedure :: new :: < GetBookRequest , :: buffa_types :: google :: protobuf :: Empty > (\"/api.Books/GetBook\")"
        ));
    }

    #[test]
    fn a_method_type_neither_generated_nor_well_known_fails_generation() {
        let request = request(&["api.proto"]);
        let index = crate::messages::gather(&request);
        let error = procedures("api", &index, &generated(&request), &Options::default())
            .expect_err("dep.Book is not generated");
        assert_eq!(
            error.to_string(),
            "api.Books/CreateBook: PROCEDURES needs dep.Book's aip::FieldBehavior impl, which is \
             generated with its file; generate that file in the same run"
        );
    }

    #[test]
    fn a_package_without_services_has_no_table() {
        let request = request(&["api.proto", "dep.proto"]);
        let index = crate::messages::gather(&request);
        let table =
            procedures("dep", &index, &generated(&request), &Options::default()).expect("emitted");
        assert!(table.is_none());
    }
}
