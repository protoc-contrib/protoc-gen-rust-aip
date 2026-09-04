//! Emits the typed resource-name surface: a struct per pattern, an enum over
//! the variants when a resource declares more than one, a typed `parent()` and
//! the matching parent-to-child builder, and the accessors that hang the whole
//! thing off the buffa-generated messages.
//!
//! Segment walking is delegated to `aip::ResourcePattern` rather than inlined,
//! so a fix to the walk reaches consumers as a dependency bump. What stays
//! generated is everything that is typed: which fields exist, what `parent()`
//! returns, which builder a parent gets.

#![allow(
    clippy::too_many_lines,
    reason = "an emitter is one template per shape; splitting it hides which \
              tokens end up next to which"
)]

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::doc;
use crate::scan::{Format, Pattern, Reference, Registry, Resource, Segment};

/// The UUID standing in for a typed segment in a doc example.
///
/// Genuinely a version 4 UUID — the `4` opening the third group, the `9`
/// opening the fourth — since the annotation being illustrated is `UUID4`.
const EXAMPLE_UUID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

/// Emits every item for the resources and references declared in one `.proto`
/// file.
///
/// Anything a schema can get wrong is rejected during the scan, so by the time
/// a resource reaches the emitter there is nothing left to fail on.
#[must_use]
pub fn emit_file(file: &str, package: &str, registry: &Registry) -> TokenStream {
    let mut items = Vec::new();
    for resource in registry.by_file(file) {
        items.push(emit_resource(resource, package, registry));
    }
    for reference in registry.references.get(file).into_iter().flatten() {
        items.push(emit_reference(reference, package, registry));
    }
    quote! { #( #items )* }
}

fn emit_resource(resource: &Resource, package: &str, registry: &Registry) -> TokenStream {
    if resource.is_multi_pattern() {
        return emit_multi_pattern(resource, package, registry);
    }
    let pattern = &resource.patterns[0];
    let name_type = format_ident!("{}", resource.name_type());
    let variant = emit_pattern_struct(resource, &name_type, pattern, package, registry, None);
    let message =
        emit_message_accessors(resource, &name_type, &quote! { ::aip::resource::ScanError });
    quote! {
        #variant
        #message
    }
}

/// A resource with more than one pattern gets a struct per pattern plus an enum
/// over them.
///
/// Go modelled this as a sealed interface; an enum is the Rust equivalent and a
/// better one — a caller can exhaustively match the patterns, which is what
/// deciding "which parent is this under?" actually needs.
fn emit_multi_pattern(resource: &Resource, package: &str, registry: &Registry) -> TokenStream {
    let name_type = format_ident!("{}", resource.name_type());
    let variants = variant_names(resource, registry);

    let mut structs = Vec::new();
    for (pattern, variant) in resource.patterns.iter().zip(&variants) {
        let variant_type = format_ident!("{}", variant);
        structs.push(emit_pattern_struct(
            resource,
            &variant_type,
            pattern,
            package,
            registry,
            Some(&name_type),
        ));
    }

    let arms: Vec<TokenStream> = resource
        .patterns
        .iter()
        .zip(&variants)
        .map(|(pattern, variant)| {
            let ident = variant_ident(variant, resource);
            let variant_type = format_ident!("{}", variant);
            let doc = doc(&format!("The `{}` pattern.", pattern.source));
            quote! { #doc #ident(#variant_type) }
        })
        .collect();

    let try_each: Vec<TokenStream> = variants
        .iter()
        .map(|variant| {
            let ident = variant_ident(variant, resource);
            let variant_type = format_ident!("{}", variant);
            quote! {
                match #variant_type::parse(name) {
                    ::core::result::Result::Ok(parsed) => {
                        return ::core::result::Result::Ok(Self::#ident(parsed));
                    }
                    ::core::result::Result::Err(error) => attempts.push(error),
                }
            }
        })
        .collect();
    let try_each_full: Vec<TokenStream> = variants
        .iter()
        .map(|variant| {
            let ident = variant_ident(variant, resource);
            let variant_type = format_ident!("{}", variant);
            quote! {
                match #variant_type::parse_full(name) {
                    ::core::result::Result::Ok(parsed) => {
                        return ::core::result::Result::Ok(Self::#ident(parsed));
                    }
                    ::core::result::Result::Err(error) => attempts.push(error),
                }
            }
        })
        .collect();

    // Every delegation is written as a fully-qualified call. The generated file
    // is included into a module this plugin does not control, so `name.fmt(f)`
    // would depend on `Display` and `ResourceName` happening to be in scope
    // there -- and being inside `impl Display` does not put `Display` in scope
    // for method resolution.
    let dispatch = |call: &dyn Fn(&TokenStream) -> TokenStream| -> Vec<TokenStream> {
        variants
            .iter()
            .map(|variant| {
                let ident = variant_ident(variant, resource);
                let body = call(&quote! { name });
                quote! { Self::#ident(name) => #body }
            })
            .collect()
    };
    let display_arms = dispatch(&|name| quote! { ::core::fmt::Display::fmt(#name, f) });
    let pattern_arms = dispatch(&|name| quote! { ::aip::ResourceName::pattern(#name) });
    let validate_arms = dispatch(&|name| quote! { ::aip::ResourceName::validate(#name) });
    let wildcard_arms = dispatch(&|name| quote! { ::aip::ResourceName::contains_wildcard(#name) });

    let conversions: Vec<TokenStream> = variants
        .iter()
        .map(|variant| {
            let ident = variant_ident(variant, resource);
            let variant_type = format_ident!("{}", variant);
            quote! {
                impl ::core::convert::From<#variant_type> for #name_type {
                    fn from(name: #variant_type) -> Self {
                        Self::#ident(name)
                    }
                }
            }
        })
        .collect();

    let count = resource.patterns.len();
    let resource_type = Literal::string(&resource.resource_type);
    let struct_doc = doc(&format!(
        "The parsed form of a `{}` resource name.\n\n\
         The resource declares {count} patterns, so a parsed name is one of them.",
        resource.resource_type,
    ));
    let parse_doc = doc(&format!(
        "Parses the relative resource name, trying each pattern of `{}` in \
         declaration order and returning the first that matches.",
        resource.resource_type,
    ));
    let parse_full_doc = doc(&format!(
        "Parses the fully-qualified resource name, prefixed `//{}/`, trying \
         each pattern in declaration order.",
        resource.domain,
    ));
    let message = emit_message_accessors(
        resource,
        &name_type,
        &quote! { ::aip::resource::NoPatternError },
    );

    quote! {
        #( #structs )*

        #struct_doc
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::core::cmp::PartialOrd,
            ::core::cmp::Ord,
            ::core::hash::Hash
        )]
        pub enum #name_type {
            #( #arms, )*
        }

        impl #name_type {
            /// The AIP resource type this name identifies.
            pub const TYPE: &'static str = #resource_type;

            #parse_doc
            pub fn parse(
                name: &str,
            ) -> ::core::result::Result<Self, ::aip::resource::NoPatternError> {
                let mut attempts = ::std::vec::Vec::with_capacity(#count);
                #( #try_each )*
                ::core::result::Result::Err(
                    ::aip::resource::NoPatternError::new(name, attempts),
                )
            }

            #parse_full_doc
            pub fn parse_full(
                name: &str,
            ) -> ::core::result::Result<Self, ::aip::resource::NoPatternError> {
                let mut attempts = ::std::vec::Vec::with_capacity(#count);
                #( #try_each_full )*
                ::core::result::Result::Err(
                    ::aip::resource::NoPatternError::new(name, attempts),
                )
            }
        }

        impl ::core::fmt::Display for #name_type {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #( #display_arms, )*
                }
            }
        }

        impl ::core::str::FromStr for #name_type {
            type Err = ::aip::resource::NoPatternError;

            fn from_str(name: &str) -> ::core::result::Result<Self, Self::Err> {
                Self::parse(name)
            }
        }

        impl ::aip::ResourceName for #name_type {
            fn resource_type(&self) -> &str {
                Self::TYPE
            }

            fn pattern(&self) -> &str {
                match self {
                    #( #pattern_arms, )*
                }
            }

            fn validate(
                &self,
            ) -> ::core::result::Result<(), ::aip::resource::InvalidResourceIdError> {
                match self {
                    #( #validate_arms, )*
                }
            }

            fn contains_wildcard(&self) -> bool {
                match self {
                    #( #wildcard_arms, )*
                }
            }
        }

        #( #conversions )*

        #message
    }
}

/// Emits the struct for one pattern, its inherent impl, `Display`, `FromStr`,
/// `aip::ResourceName`, and — when the pattern is nested under a resource the
/// request also declares — `parent()` plus the builder on that parent.
///
/// `enclosing` is the enum this struct is a variant of, for a multi-pattern
/// resource; it only changes the doc comments.
fn emit_pattern_struct(
    resource: &Resource,
    name_type: &proc_macro2::Ident,
    pattern: &Pattern,
    package: &str,
    registry: &Registry,
    enclosing: Option<&proc_macro2::Ident>,
) -> TokenStream {
    let segments: Vec<&Segment> = pattern.variables().collect();
    let arity = segments.len();
    let fields: Vec<proc_macro2::Ident> = segments.iter().map(|s| id_field(&s.name)).collect();

    let field_decls: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .map(|(segment, field)| {
            let doc = doc(&match segment.format {
                Format::String => format!("The `{{{}}}` segment.", segment.name),
                Format::Uuid => format!(
                    "The `{{{}}}` segment, a UUID by `google.api.field_info` on \
                     the create request that mints it.",
                    segment.name,
                ),
            });
            let ty = segment_type(segment.format);
            quote! { #doc pub #field: #ty }
        })
        .collect();

    // A scan only ever yields `&str`, so a segment declared as something
    // narrower is converted here, and a failed conversion is reported as one
    // more way the name does not match the pattern.
    let assignments: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .enumerate()
        .map(|(index, (segment, field))| {
            let index = Literal::usize_unsuffixed(index);
            match segment.format {
                Format::String => {
                    quote! { #field: ::std::borrow::ToOwned::to_owned(ids[#index]) }
                }
                Format::Uuid => {
                    let variable = Literal::string(&segment.name);
                    quote! {
                        #field: match <::uuid::Uuid as ::core::str::FromStr>::from_str(ids[#index]) {
                            ::core::result::Result::Ok(value) => value,
                            ::core::result::Result::Err(error) => {
                                return ::core::result::Result::Err(
                                    Self::compiled().invalid_value(name, #variable, error),
                                );
                            }
                        }
                    }
                }
            }
        })
        .collect();

    let display = display_impl(name_type, pattern, &fields);

    // A pattern with no variables is a singleton: there are no ids to check,
    // and the empty slice needs a type for inference.
    let ids: TokenStream = if arity == 0 {
        quote! { let ids: [&str; 0] = []; }
    } else {
        let arity = Literal::usize_unsuffixed(arity);
        quote! { let mut ids = [""; #arity]; }
    };
    let ids_ref = if arity == 0 {
        quote! { &ids }
    } else {
        quote! { &mut ids }
    };
    // Validated a segment at a time rather than through
    // `ResourcePattern::validate`, which needs every value as a string and so
    // has nothing to say about a typed one.
    let checks: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .map(|(segment, field)| {
            let variable = Literal::string(&segment.name);
            match segment.format {
                Format::String => quote! {
                    ::aip::resource::validate_segment(#variable, &self.#field)?;
                },
                // A UUID cannot be empty or hold a `/`; the nil UUID is the
                // degenerate value worth rejecting in its place.
                Format::Uuid => quote! {
                    if ::uuid::Uuid::is_nil(&self.#field) {
                        return ::core::result::Result::Err(
                            ::aip::resource::InvalidResourceIdError::empty(#variable),
                        );
                    }
                },
            }
        })
        .collect();

    // Only a string segment can be the wildcard -- no UUID parses from `-`.
    let wildcards: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .filter(|(segment, _)| segment.format == Format::String)
        .map(|(_, field)| quote! { self.#field == ::aip::resource::WILDCARD })
        .collect();
    let wildcard = if wildcards.is_empty() {
        quote! { false }
    } else {
        quote! { #( #wildcards )||* }
    };

    let parent = emit_parent(resource, name_type, pattern, package, registry);
    let parent_method = parent.as_ref().map(|p| p.method.clone());
    let parent_builder = parent.as_ref().map(|p| p.builder.clone());

    let resource_type = Literal::string(&resource.resource_type);
    let pattern_literal = Literal::string(&pattern.source);
    let domain = Literal::string(&resource.domain);
    let invalid = Literal::string(&format!(
        "protoc-gen-rust-aip emitted an invalid pattern for {}",
        resource.resource_type,
    ));

    let struct_doc = match enclosing {
        Some(enclosing) => doc(&format!(
            "The `{}` variant of [`{enclosing}`].\n\n\
             Pattern: `{}`.",
            pattern.source, pattern.source,
        )),
        None => doc(&format!(
            "The parsed form of a `{}` resource name.\n\n\
             Pattern: `{}`.",
            resource.resource_type, pattern.source,
        )),
    };
    let parse_doc = doc(&format!(
        "Parses the relative resource name, e.g. `{}`.",
        example(pattern),
    ));
    let parse_full_doc = doc(&format!(
        "Parses the fully-qualified resource name, e.g. `//{}/{}`.",
        resource.domain,
        example(pattern),
    ));

    quote! {
        #struct_doc
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
            ::core::default::Default,
            ::core::cmp::PartialEq,
            ::core::cmp::Eq,
            ::core::cmp::PartialOrd,
            ::core::cmp::Ord,
            ::core::hash::Hash
        )]
        pub struct #name_type {
            #( #field_decls, )*
        }

        impl #name_type {
            /// The AIP resource type this name identifies.
            pub const TYPE: &'static str = #resource_type;
            /// The pattern this name is parsed against and rendered from.
            pub const PATTERN: &'static str = #pattern_literal;

            /// The pattern, compiled once on first use.
            ///
            /// A pattern that does not compile is a codegen bug rather than
            /// anything a request can provoke, which is what makes `expect`
            /// right here.
            fn compiled() -> &'static ::aip::ResourcePattern {
                static COMPILED: ::std::sync::LazyLock<::aip::ResourcePattern> =
                    ::std::sync::LazyLock::new(|| {
                        ::core::str::FromStr::from_str(#pattern_literal).expect(#invalid)
                    });
                &COMPILED
            }

            #parse_doc
            pub fn parse(
                name: &str,
            ) -> ::core::result::Result<Self, ::aip::resource::ScanError> {
                #ids
                Self::compiled().scan_into(name, #ids_ref)?;
                ::core::result::Result::Ok(Self { #( #assignments, )* })
            }

            #parse_full_doc
            pub fn parse_full(
                name: &str,
            ) -> ::core::result::Result<Self, ::aip::resource::ScanError> {
                #ids
                Self::compiled().scan_full_into(name, #domain, #ids_ref)?;
                ::core::result::Result::Ok(Self { #( #assignments, )* })
            }

            #parent_method
        }

        #display

        impl ::core::str::FromStr for #name_type {
            type Err = ::aip::resource::ScanError;

            fn from_str(name: &str) -> ::core::result::Result<Self, Self::Err> {
                Self::parse(name)
            }
        }

        impl ::aip::ResourceName for #name_type {
            fn resource_type(&self) -> &str {
                Self::TYPE
            }

            fn pattern(&self) -> &str {
                Self::PATTERN
            }

            fn validate(
                &self,
            ) -> ::core::result::Result<(), ::aip::resource::InvalidResourceIdError> {
                #( #checks )*
                ::core::result::Result::Ok(())
            }

            fn contains_wildcard(&self) -> bool {
                #wildcard
            }
        }

        #parent_builder
    }
}

/// `Display` writes the relative name, which is the form that travels in a
/// `name` field.
fn display_impl(
    name_type: &proc_macro2::Ident,
    pattern: &Pattern,
    fields: &[proc_macro2::Ident],
) -> TokenStream {
    // Built as a format string over the pattern with `{var}` replaced by `{}`,
    // so the whole name is one `write!` rather than a chain of pushes.
    let mut template = String::new();
    for (index, segment) in pattern.segments.iter().enumerate() {
        if index > 0 {
            template.push('/');
        }
        if segment.variable {
            template.push_str("{}");
        } else {
            template.push_str(&segment.name);
        }
    }
    let template = Literal::string(&template);
    let args: Vec<TokenStream> = fields.iter().map(|field| quote! { self.#field }).collect();
    quote! {
        impl ::core::fmt::Display for #name_type {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::write!(f, #template #(, #args )*)
            }
        }
    }
}

/// The `parent()` method and the matching builder on the parent, when the
/// pattern is nested under a resource the request declares.
struct ParentBinding {
    method: TokenStream,
    builder: TokenStream,
}

fn emit_parent(
    resource: &Resource,
    name_type: &proc_macro2::Ident,
    pattern: &Pattern,
    package: &str,
    registry: &Registry,
) -> Option<ParentBinding> {
    // AIP-122 records no parent link, so the only evidence of one is that some
    // resource declares exactly the pattern this one is nested in.
    let parent_pattern = pattern.parent()?;
    let (parent, index) = registry.find_by_pattern(&parent_pattern)?;

    let parent_variants = variant_names(parent, registry);
    let parent_variant = if parent.is_multi_pattern() {
        parent_variants[index].clone()
    } else {
        parent.name_type()
    };
    let variant_path = type_path(package, parent, &parent_variant);

    // `parent()` returns the resource's public type -- the enum for a
    // multi-pattern parent -- but builds the one variant that matched.
    let return_path = type_path(package, parent, &parent.name_type());
    let matched = &parent.patterns[index];
    let parent_fields: Vec<proc_macro2::Ident> =
        matched.variables().map(|s| id_field(&s.name)).collect();
    let carried: Vec<TokenStream> = parent_fields
        .iter()
        .map(|field| quote! { #field: ::core::clone::Clone::clone(&self.#field) })
        .collect();
    let construct = quote! { #variant_path { #( #carried, )* } };
    let construct = if parent.is_multi_pattern() {
        quote! { ::core::convert::From::from(#construct) }
    } else {
        construct
    };

    let method_doc = doc(&format!(
        "The `{}` this resource belongs to.",
        parent.resource_type,
    ));
    let method = quote! {
        #method_doc
        pub fn parent(&self) -> #return_path {
            #construct
        }
    };

    // The child's own segments -- everything the parent does not already carry.
    let own: Vec<&Segment> = pattern.variables().skip(parent_fields.len()).collect();
    let own_fields: Vec<proc_macro2::Ident> = own.iter().map(|s| id_field(&s.name)).collect();
    // A string segment takes anything that converts; a typed one takes the type
    // itself, so a caller holding a `Uuid` never bridges through a string.
    let params: Vec<TokenStream> = own
        .iter()
        .zip(&own_fields)
        .map(|(segment, field)| match segment.format {
            Format::String => {
                quote! { #field: impl ::core::convert::Into<::std::string::String> }
            }
            Format::Uuid => quote! { #field: ::uuid::Uuid },
        })
        .collect();
    let from_parent: Vec<TokenStream> = parent_fields
        .iter()
        .map(|field| quote! { #field: ::core::clone::Clone::clone(&self.#field) })
        .collect();
    let from_args: Vec<TokenStream> = own
        .iter()
        .zip(&own_fields)
        .map(|(segment, field)| match segment.format {
            Format::String => quote! { #field: ::core::convert::Into::into(#field) },
            Format::Uuid => quote! { #field },
        })
        .collect();

    let builder_name = format_ident!("{}", snake_case(&name_type.to_string()));
    let builder_doc = doc(&format!(
        "Builds the name of a `{}` under this `{}`.",
        resource.resource_type, parent.resource_type,
    ));
    // The impl block is emitted in the child's file, so the parent is named by
    // path and the child by its local name -- the reverse of `parent()`.
    let builder = quote! {
        impl #variant_path {
            #builder_doc
            pub fn #builder_name(&self #(, #params )*) -> #name_type {
                #name_type {
                    #( #from_parent, )*
                    #( #from_args, )*
                }
            }
        }
    };

    Some(ParentBinding { method, builder })
}

/// The accessors hung off the buffa-generated message that carries the name.
fn emit_message_accessors(
    resource: &Resource,
    name_type: &proc_macro2::Ident,
    error: &TokenStream,
) -> TokenStream {
    let Some(binding) = &resource.message else {
        return TokenStream::new();
    };
    let message: TokenStream = binding
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let field = buffa_codegen::idents::make_field_ident(&binding.name_field);
    let value = if binding.name_field_optional {
        quote! { self.#field.as_deref().unwrap_or_default() }
    } else {
        quote! { &self.#field }
    };
    let parse_doc = doc(&format!(
        "Parses `{}` as a `{}` resource name.",
        binding.name_field, resource.resource_type,
    ));
    let parse_full_doc = doc(&format!(
        "Parses `{}` as a fully-qualified `{}` resource name.",
        binding.name_field, resource.resource_type,
    ));
    quote! {
        impl #message {
            #parse_doc
            pub fn parse_name(&self) -> ::core::result::Result<#name_type, #error> {
                #name_type::parse(#value)
            }

            #parse_full_doc
            pub fn parse_full_name(&self) -> ::core::result::Result<#name_type, #error> {
                #name_type::parse_full(#value)
            }
        }
    }
}

/// The accessor for a `google.api.resource_reference` field: the referring
/// message learns to parse its own field as the referent's name type.
fn emit_reference(reference: &Reference, package: &str, registry: &Registry) -> TokenStream {
    let resource = &registry.resources[reference.resource];
    let message: TokenStream = reference
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let field = buffa_codegen::idents::make_field_ident(&reference.field_name);
    let method = format_ident!("parse_{}", reference.field_name);
    let name_path = type_path(package, resource, &resource.name_type());
    let error = if resource.is_multi_pattern() {
        quote! { ::aip::resource::NoPatternError }
    } else {
        quote! { ::aip::resource::ScanError }
    };
    let value = if reference.field_optional {
        quote! { self.#field.as_deref().unwrap_or_default() }
    } else {
        quote! { &self.#field }
    };
    let method_doc = doc(&format!(
        "Parses `{}` as the `{}` resource name it references.",
        reference.field_name, resource.resource_type,
    ));
    quote! {
        impl #message {
            #method_doc
            pub fn #method(&self) -> ::core::result::Result<#name_path, #error> {
                #name_path::parse(#value)
            }
        }
    }
}

/// The Rust type names for each pattern of a multi-pattern resource.
///
/// A variant whose pattern is nested under a resource the request declares is
/// named after that parent — `BookName` under `Publisher` becomes
/// `PublisherBookName` — because that is what tells the two apart at a call
/// site. Where that is unavailable or ambiguous, the pattern's index is the
/// only thing left that distinguishes them.
fn variant_names(resource: &Resource, registry: &Registry) -> Vec<String> {
    let mut candidates: Vec<Option<String>> = Vec::with_capacity(resource.patterns.len());
    for pattern in &resource.patterns {
        candidates.push(
            pattern
                .parent()
                .and_then(|parent| registry.find_by_pattern(&parent).map(|(r, _)| r))
                .map(|parent| format!("{}{}Name", parent.type_name, resource.type_name)),
        );
    }
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| match candidate {
            Some(name)
                if candidates
                    .iter()
                    .filter(|c| c.as_ref() == Some(name))
                    .count()
                    == 1 =>
            {
                name.clone()
            }
            _ => format!("{}NamePattern{index}", resource.type_name),
        })
        .collect()
}

/// The enum variant for a variant type name: the type without its `Name`
/// suffix, or `Pattern{N}` for an index-named fallback.
fn variant_ident(variant: &str, resource: &Resource) -> proc_macro2::Ident {
    let fallback = format!("{}NamePattern", resource.type_name);
    if let Some(index) = variant.strip_prefix(&fallback) {
        return format_ident!("Pattern{}", index);
    }
    format_ident!("{}", variant.strip_suffix("Name").unwrap_or(variant))
}

/// The path by which code emitted for `from_package` names a type belonging to
/// `resource`'s package.
///
/// Within one package the short name is enough. Across packages the path walks
/// up to the root of the emitted module tree and back down, which works
/// wherever the consumer mounts that tree — an absolute `crate::` path would
/// have to be configured, and would be wrong the moment it was not.
fn type_path(from_package: &str, resource: &Resource, type_name: &str) -> TokenStream {
    let ident = format_ident!("{}", type_name);
    if resource.package == from_package {
        return quote! { #ident };
    }
    let ups = std::iter::repeat_n(quote! { super }, package_depth(from_package));
    let downs = resource
        .package
        .split('.')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let segment = format_ident!("{}", buffa_codegen::idents::escape_mod_ident(segment));
            quote! { #segment }
        });
    quote! { #( #ups :: )* #( #downs :: )* #ident }
}

fn package_depth(package: &str) -> usize {
    if package.is_empty() {
        0
    } else {
        package.split('.').count()
    }
}

/// The Rust type a variable segment is stored as.
///
/// A `Uuid` costs the consumer a `uuid` dependency, but only a schema that
/// annotates an ID as UUID4 pays it -- nothing else in the output names the
/// crate.
fn segment_type(format: Format) -> TokenStream {
    match format {
        Format::String => quote! { ::std::string::String },
        Format::Uuid => quote! { ::uuid::Uuid },
    }
}

/// The struct field for a variable segment: `{publisher}` becomes
/// `publisher_id`.
fn id_field(segment: &str) -> proc_macro2::Ident {
    buffa_codegen::idents::make_field_ident(&format!("{}_id", snake_case(segment)))
}

/// A sample name for the pattern, for the doc comment on `parse`. Each variable
/// segment is filled with the first letter of its name and a `1`, which is the
/// convention AIP examples use: `publishers/p1/books/b1`.
fn example(pattern: &Pattern) -> String {
    let mut out = String::new();
    for (index, segment) in pattern.segments.iter().enumerate() {
        if index > 0 {
            out.push('/');
        }
        if segment.variable {
            match segment.format {
                // The AIP examples' convention: the segment's initial and a 1,
                // as in `publishers/p1/books/b1`.
                Format::String => {
                    out.push(segment.name.chars().next().unwrap_or('x'));
                    out.push('1');
                }
                // Nothing that short is a UUID, and a doc example showing one
                // as `o1` teaches the reader the wrong shape.
                Format::Uuid => out.push_str(EXAMPLE_UUID),
            }
        } else {
            out.push_str(&segment.name);
        }
    }
    out
}

/// `PascalCase` to `snake_case`, for deriving a method name from a type name.
fn snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 2);
    for (index, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && index > 0 {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if previous.is_lowercase() || (previous.is_uppercase() && next_is_lower) {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
