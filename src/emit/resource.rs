//! Emits the typed resource-name surface: a struct per pattern, an enum over
//! the variants when a resource declares more than one, a typed `parent()` and
//! the matching parent-to-child builder, and the accessors that hang the whole
//! thing off the buffa-generated messages.
//!
//! The segment walk is emitted inline, as protoc-gen-go-aip emits it: a
//! `parse` that splits on `/` and checks each segment in turn, with the words
//! aip-go uses for a failure, returning `aip::resource::ResourceNameError`.
//! What the runtime holds is only what is shared -- the error, the trait a name
//! satisfies, the wildcard -- so a fix to the walk reaches a consumer as a
//! regeneration, and a consumer's name types depend on nothing they cannot
//! read.

#![allow(
    clippy::too_many_lines,
    reason = "an emitter is one template per shape; splitting it hides which \
              tokens end up next to which"
)]

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::{Options, doc, impl_for, package_depth, short_name};
use crate::idents::snake_case;
use crate::messages::{Index, Kind};
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
pub fn emit_file(file: &str, package: &str, registry: &Registry, options: &Options) -> TokenStream {
    let views = options.views;
    let mut items = Vec::new();
    for resource in registry.by_file(file) {
        items.push(emit_resource(resource, package, registry, views));
    }
    for reference in registry.references.get(file).into_iter().flatten() {
        items.push(emit_reference(reference, package, registry, options));
    }
    quote! { #( #items )* }
}

fn emit_resource(
    resource: &Resource,
    package: &str,
    registry: &Registry,
    views: bool,
) -> TokenStream {
    if resource.is_multi_pattern() {
        return emit_multi_pattern(resource, package, registry, views);
    }
    let pattern = &resource.patterns[0];
    let name_type = format_ident!("{}", resource.name_type());
    let variant = emit_pattern_struct(resource, &name_type, pattern, package, registry, None);
    let message = emit_message_accessors(resource, &name_type, views);
    quote! {
        #variant
        #message
    }
}

/// The error every parse and validation of a name fails with.
fn name_error() -> TokenStream {
    quote! { ::aip::resource::ResourceNameError }
}

/// `Err(ResourceNameError::new(<message>))`, where `message` is the arguments of
/// a `format!`.
fn failure(message: &TokenStream) -> TokenStream {
    let error = name_error();
    quote! {
        ::core::result::Result::Err(#error::new(::std::format!(#message)))
    }
}

/// [`failure`] as a statement that returns it, for a check part-way through a
/// function. A tail position has no use for the `return`, which clippy's
/// `needless_return` would say.
fn fail(message: &TokenStream) -> TokenStream {
    let failure = failure(message);
    quote! { return #failure; }
}

/// The `parse_full` of a name type: the service domain's `//` prefix, then
/// whatever `parse` makes of the rest.
fn parse_full_body(domain: &str) -> TokenStream {
    let prefix = format!("//{domain}/");
    let refuse = fail(&quote! { "parse {name:?}: invalid prefix, want: {:?}", #prefix });
    quote! {
        let ::core::option::Option::Some(relative) = name.strip_prefix(#prefix) else {
            #refuse
        };
        Self::parse(relative)
    }
}

/// A resource with more than one pattern gets a struct per pattern plus an enum
/// over them.
///
/// Go modelled this as a sealed interface; an enum is the Rust equivalent and a
/// better one — a caller can exhaustively match the patterns, which is what
/// deciding "which parent is this under?" actually needs.
fn emit_multi_pattern(
    resource: &Resource,
    package: &str,
    registry: &Registry,
    views: bool,
) -> TokenStream {
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
    let contains_arms = dispatch(&|name| quote! { #name.contains_wildcard() });
    let is_arms = dispatch(&|name| quote! { #name.is_wildcard() });

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
    let error = name_error();
    let struct_doc = doc(&format!(
        "The parsed form of a `{}` resource name.\n\n\
         The resource declares {count} patterns, so a parsed name is one of them.",
        resource.resource_type,
    ));
    let parse_doc = doc(&format!(
        "Parses the relative resource name, trying each pattern of `{}` in \
         declaration order and returning the first that matches.\n\n\
         # Errors\n\n\
         If `name` matches none of the {count} patterns. The error carries \
         what each one rejected, so the caller can say more than \"no\".",
        resource.resource_type,
    ));
    let parse_full_doc = doc(&format!(
        "Parses the fully-qualified resource name, prefixed `//{}/`, and \
         then as [`parse`](Self::parse) does.\n\n\
         # Errors\n\n\
         If `name` does not carry the prefix, or what follows it matches none \
         of the {count} patterns.",
        resource.domain,
    ));
    let no_pattern = failure(&quote! {
        "parse {name:?}: no pattern matches: {}",
        attempts
            .iter()
            .map(::std::string::ToString::to_string)
            .collect::<::std::vec::Vec<_>>()
            .join("\n")
    });
    let parse_full = parse_full_body(&resource.domain);
    let message = emit_message_accessors(resource, &name_type, views);

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
            pub fn parse(name: &str) -> ::core::result::Result<Self, #error> {
                let mut attempts: ::std::vec::Vec<#error> =
                    ::std::vec::Vec::with_capacity(#count);
                #( #try_each )*
                #no_pattern
            }

            #parse_full_doc
            pub fn parse_full(name: &str) -> ::core::result::Result<Self, #error> {
                #parse_full
            }

            /// Whether any variable segment is the AIP-159 wildcard.
            #[must_use]
            pub fn contains_wildcard(&self) -> bool {
                match self {
                    #( #contains_arms, )*
                }
            }

            /// Whether every variable segment is the AIP-159 wildcard.
            #[must_use]
            pub fn is_wildcard(&self) -> bool {
                match self {
                    #( #is_arms, )*
                }
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
            type Err = #error;

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

            fn validate(&self) -> ::core::result::Result<(), #error> {
                match self {
                    #( #validate_arms, )*
                }
            }
        }

        #( #conversions )*

        #message
    }
}

/// `return Err(refuse(format_args!(<message>)))`: a `parse` that fails, with
/// what is wrong with the name, which `refuse` -- see [`parse_checks`] -- puts
/// after the `parse "<name>": ` every such failure begins with.
fn refuse(message: &TokenStream) -> TokenStream {
    quote! {
        return ::core::result::Result::Err(refuse(::core::format_args!(#message)));
    }
}

/// The segment checks of a pattern's `parse`: the number of segments, then one
/// check per segment, in the words aip-go uses.
///
/// A segment index is the pattern's own, literals counted, as aip-go's is.
fn parse_checks(pattern: &Pattern) -> TokenStream {
    let count = Literal::usize_unsuffixed(pattern.segments.len());
    let wrong_count = refuse(&quote! {
        "bad number of segments, want: {}, got: {}",
        #count,
        parts.len()
    });
    let checks: Vec<TokenStream> = pattern
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let at = Literal::usize_unsuffixed(index);
            if !segment.variable {
                let literal = &segment.name;
                let wrong = refuse(&quote! {
                    "bad segment {}, want: {:?}, got: {:?}",
                    #at,
                    #literal,
                    parts[#at]
                });
                return quote! {
                    if parts[#at] != #literal {
                        #wrong
                    }
                };
            }
            let empty = refuse(&quote! { "empty value for segment {}", #at });
            let canonical = match segment.format {
                Format::String => TokenStream::new(),
                // A UUID segment is a canonical UUID, or the AIP-159 wildcard,
                // which is why the field is a string: a `Uuid` cannot hold it.
                // `uuid` also reads uppercase, braced, `urn:uuid:` and
                // unhyphenated text, which a name read from would not
                // round-trip (AIP-122), so only the canonical lowercase,
                // hyphenated form -- the one it formats to -- is a segment.
                Format::Uuid => {
                    let uncanonical = refuse(&quote! {
                        "segment {}: not a canonical lowercase UUID", #at
                    });
                    quote! {
                        if parts[#at] != ::aip::resource::RESOURCE_WILDCARD {
                            let id = <::uuid::Uuid as ::core::str::FromStr>::from_str(parts[#at])
                                .map_err(|error| {
                                    refuse(::core::format_args!("segment {}: {error}", #at))
                                })?;
                            if ::std::string::ToString::to_string(&id) != parts[#at] {
                                #uncanonical
                            }
                        }
                    }
                }
            };
            quote! {
                if parts[#at].is_empty() {
                    #empty
                }
                #canonical
            }
        })
        .collect();
    let error = name_error();
    quote! {
        let refuse = |reason: ::core::fmt::Arguments<'_>| {
            #error::new(::std::format!("parse {name:?}: {reason}"))
        };
        let parts: ::std::vec::Vec<&str> = name.split('/').collect();
        if parts.len() != #count {
            #wrong_count
        }
        #( #checks )*
    }
}

/// The checks of a pattern's `validate`: every variable segment is usable as the
/// id of a resource.
///
/// A string segment is non-empty and holds no `/`. A UUID segment is a
/// canonical, non-nil UUID, which the wildcard is not: it is a name to read
/// across, not a resource to write.
fn validate_checks(segments: &[&Segment], fields: &[proc_macro2::Ident]) -> Vec<TokenStream> {
    segments
        .iter()
        .zip(fields)
        .map(|(segment, field)| match segment.format {
            Format::String => {
                let empty = fail_with(&format!("{}: empty", segment.name));
                let separator = fail_with(&format!(
                    "{}: contains illegal character '/'",
                    segment.name
                ));
                quote! {
                    if self.#field.is_empty() {
                        #empty
                    }
                    if self.#field.contains('/') {
                        #separator
                    }
                }
            }
            Format::Uuid => {
                let refused = fail_with(&format!("{}: not a canonical non-nil UUID", segment.name));
                quote! {
                    match <::uuid::Uuid as ::core::str::FromStr>::from_str(&self.#field) {
                        ::core::result::Result::Ok(id)
                            if !id.is_nil() && ::std::string::ToString::to_string(&id) == self.#field => {}
                        _ => {
                            #refused
                        }
                    }
                }
            }
        })
        .collect()
}

/// `return Err(ResourceNameError::new("<message>"))`, for a message with nothing
/// left to format.
fn fail_with(message: &str) -> TokenStream {
    let error = name_error();
    quote! {
        return ::core::result::Result::Err(#error::new(#message));
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
    let fields: Vec<proc_macro2::Ident> = segments.iter().map(|s| id_field(&s.name)).collect();

    // Every segment is a `String`, a typed one included, so that it can hold
    // the AIP-159 wildcard `-`, which a `Uuid` cannot; the typed id comes from
    // the `try_<segment>_id` accessor, as aip-go's from `Parse<Segment>ID`.
    let field_decls: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .map(|(segment, field)| {
            let doc = doc(&match segment.format {
                Format::String => format!("The `{{{}}}` segment.", segment.name),
                Format::Uuid => format!(
                    "The `{{{}}}` segment, a UUID by `google.api.field_info` on \
                     the create request that mints it, or the AIP-159 wildcard.",
                    segment.name,
                ),
            });
            quote! { #doc pub #field: ::std::string::String }
        })
        .collect();

    // The values, taken from the parts `parse_checks` has walked: the
    // variable segments are the parts at the indices that are not literals.
    let assignments: Vec<TokenStream> = pattern
        .segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| segment.variable)
        .zip(&fields)
        .map(|((index, _), field)| {
            let index = Literal::usize_unsuffixed(index);
            quote! { #field: ::std::borrow::ToOwned::to_owned(parts[#index]) }
        })
        .collect();

    let display = display_impl(name_type, pattern, &fields);
    let parse_checks = parse_checks(pattern);
    let parse_full = parse_full_body(&resource.domain);
    let checks = validate_checks(&segments, &fields);

    // A segment is the wildcard if it reads `-`.
    let wildcards: Vec<TokenStream> = fields
        .iter()
        .map(|field| quote! { self.#field == ::aip::resource::RESOURCE_WILDCARD })
        .collect();
    let (any_wildcard, every_wildcard) = if wildcards.is_empty() {
        (quote! { false }, quote! { false })
    } else {
        (quote! { #( #wildcards )||* }, quote! { #( #wildcards )&&* })
    };

    // The id of a typed segment, which the wildcard is none of: aip-go's
    // `ParseItemID`, named `try_` and for the field the id is in: `try_item_id`
    // beside the `item_id` field.
    let id_accessors: Vec<TokenStream> = segments
        .iter()
        .zip(&fields)
        .filter(|(segment, _)| segment.format == Format::Uuid)
        .map(|(segment, field)| {
            let accessor = format_ident!("try_{}", field);
            let wildcard = fail_with(&format!("{}: the wildcard is not an id", segment.name));
            let unreadable = format!("{}: {{error}}", segment.name);
            let error = name_error();
            let accessor_doc = doc(&format!(
                "The `{{{}}}` id. Fails for the wildcard, which is no id.",
                segment.name,
            ));
            quote! {
                #accessor_doc
                ///
                /// # Errors
                ///
                /// If the segment is the wildcard, or not a UUID.
                pub fn #accessor(&self) -> ::core::result::Result<::uuid::Uuid, #error> {
                    if self.#field == ::aip::resource::RESOURCE_WILDCARD {
                        #wildcard
                    }
                    <::uuid::Uuid as ::core::str::FromStr>::from_str(&self.#field)
                        .map_err(|error| #error::new(::std::format!(#unreadable)))
                }
            }
        })
        .collect();

    let parent = emit_parent(resource, name_type, pattern, package, registry);
    let parent_method = parent.as_ref().map(|p| p.method.clone());
    let parent_builder = parent.as_ref().map(|p| p.builder.clone());

    let resource_type = Literal::string(&resource.resource_type);
    let pattern_literal = Literal::string(&pattern.source);
    let error = name_error();

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
    // Every variable segment that is not a plain string can also reject a
    // value the scan accepted, so the `# Errors` section says so only when the
    // pattern actually has one.
    let typed = segments
        .iter()
        .any(|segment| segment.format != Format::String);
    let value_clause = if typed {
        ", or a segment whose value is not of the type that segment holds"
    } else {
        ""
    };
    let parse_doc = doc(&format!(
        "Parses the relative resource name, e.g. `{}`.\n\n\
         # Errors\n\n\
         If `name` is not of the pattern `{}`: the wrong number of segments, a \
         literal segment that does not match, an empty variable segment{}.",
        example(pattern),
        pattern.source,
        value_clause,
    ));
    let parse_full_doc = doc(&format!(
        "Parses the fully-qualified resource name, e.g. `//{}/{}`.\n\n\
         # Errors\n\n\
         If `name` does not begin `//{}/`, or the rest of it is not of the \
         pattern `{}`; see [`parse`](Self::parse).",
        resource.domain,
        example(pattern),
        resource.domain,
        pattern.source,
    ));

    quote! {
        #struct_doc
        // No `Default`: every variable segment would be empty, which is a name
        // `validate` rejects. A type whose default value is invalid is a trap,
        // and a struct literal is no harder to write.
        #[derive(
            ::core::fmt::Debug,
            ::core::clone::Clone,
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

            #parse_doc
            pub fn parse(name: &str) -> ::core::result::Result<Self, #error> {
                #parse_checks
                ::core::result::Result::Ok(Self { #( #assignments, )* })
            }

            #parse_full_doc
            pub fn parse_full(name: &str) -> ::core::result::Result<Self, #error> {
                #parse_full
            }

            #parent_method

            /// Whether any variable segment is the AIP-159 wildcard.
            #[must_use]
            pub fn contains_wildcard(&self) -> bool {
                #any_wildcard
            }

            /// Whether every variable segment is the AIP-159 wildcard.
            #[must_use]
            pub fn is_wildcard(&self) -> bool {
                #every_wildcard
            }

            #( #id_accessors )*
        }

        #display

        impl ::core::str::FromStr for #name_type {
            type Err = #error;

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

            fn validate(&self) -> ::core::result::Result<(), #error> {
                #( #checks )*
                ::core::result::Result::Ok(())
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

/// The field initializer that carries `field` over from `self`.
fn carry(field: &proc_macro2::Ident) -> TokenStream {
    quote! { #field: ::core::clone::Clone::clone(&self.#field) }
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
    let (parent, index) = registry.find_by_pattern(&parent_pattern, &resource.package)?;

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
    // The segments the parent carries, read off this resource.
    let carried: Vec<TokenStream> = parent_fields.iter().map(carry).collect();
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
        #[must_use]
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
    let from_parent: Vec<TokenStream> = parent_fields.iter().map(carry).collect();
    let from_args: Vec<TokenStream> = own
        .iter()
        .zip(&own_fields)
        .map(|(segment, field)| match segment.format {
            Format::String => quote! { #field: ::core::convert::Into::into(#field) },
            Format::Uuid => quote! { #field: ::std::string::ToString::to_string(&#field) },
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
            #[must_use]
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
    views: bool,
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
        "Parses `{}` as a `{}` resource name.\n\n\
         # Errors\n\n\
         If the field does not hold a name of this resource; see \
         [`{name_type}::parse`], which the [`FieldError`](::aip::FieldError) \
         names `{}` for.",
        binding.name_field, resource.resource_type, binding.name_field,
    ));
    let parse_full_doc = doc(&format!(
        "Parses `{}` as a fully-qualified `{}` resource name.\n\n\
         # Errors\n\n\
         If the field does not hold a fully-qualified name of this resource; \
         see [`{name_type}::parse_full`], which the \
         [`FieldError`](::aip::FieldError) names `{}` for.",
        binding.name_field, resource.resource_type, binding.name_field,
    ));
    let at = field_at(&binding.name_field, binding.name_field_number);
    impl_for(
        &message,
        views,
        &quote! {
            #parse_doc
            pub fn parse_name(
                &self,
            ) -> ::core::result::Result<#name_type, ::aip::FieldError> {
                #name_type::parse(#value).map_err(|error| ::aip::FieldError::new(#at, error))
            }

            #parse_full_doc
            pub fn parse_full_name(
                &self,
            ) -> ::core::result::Result<#name_type, ::aip::FieldError> {
                #name_type::parse_full(#value).map_err(|error| ::aip::FieldError::new(#at, error))
            }
        },
    )
}

/// Whether a reference is the `name` or `parent` of a request: the resource
/// name a standard method (AIP-131 to AIP-135) acts on.
///
/// That name is a key -- a wildcard or a non-canonical id is no name to act
/// on -- so its parse also validates it, unless the request is one of
/// [`wildcard_requests`](Options::wildcard_requests). Any other reference may
/// carry a wildcard.
fn is_request_name(reference: &Reference) -> bool {
    matches!(reference.field_name.as_str(), "name" | "parent")
}

/// The accessor for a `google.api.resource_reference` field: the referring
/// message learns to parse its own field as the referent's name type.
fn emit_reference(
    reference: &Reference,
    package: &str,
    registry: &Registry,
    options: &Options,
) -> TokenStream {
    let resource = &registry.resources[reference.resource];
    let message: TokenStream = reference
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let field = buffa_codegen::idents::make_field_ident(&reference.field_name);
    let method = format_ident!("parse_{}", reference.field_name);
    let name_path = type_path(package, resource, &resource.name_type());
    let value = if reference.field_optional {
        quote! { self.#field.as_deref().unwrap_or_default() }
    } else {
        quote! { &self.#field }
    };

    let listed = options.wildcard_requests.contains(&reference.proto_name);
    let validate = is_request_name(reference) && !listed;
    // A listed request's parents may be the wildcard, but not the resource its
    // `name` names, which is then no resource at all: its own segment, the last
    // variable one, may not. Only for a resource of one pattern, since a name
    // of several is an enum, with no segment to read.
    let own = if listed && reference.field_name == "name" && !resource.is_multi_pattern() {
        resource.patterns[0].variables().last()
    } else {
        None
    };

    let at = field_at(&reference.field_name, reference.field_number);
    let error = name_error();
    let validated = validate.then(|| {
        quote! { ::aip::ResourceName::validate(&parsed).map_err(fail)?; }
    });
    let owned = own.map(|segment| {
        let own_field = id_field(&segment.name);
        match segment.format {
            // The id accessor fails for the wildcard, which is no id.
            Format::Uuid => {
                let accessor = format_ident!("try_{own_field}");
                quote! { parsed.#accessor().map_err(fail)?; }
            }
            Format::String => {
                let refused = format!("{}: the wildcard is not an id", segment.name);
                quote! {
                    if parsed.#own_field == ::aip::resource::RESOURCE_WILDCARD {
                        return ::core::result::Result::Err(fail(#error::new(#refused)));
                    }
                }
            }
        }
    });
    let checked = validated.is_some() || owned.is_some();
    // A reference that only parses has nothing to bind a name for.
    let body = if checked {
        quote! {
            let fail = |error: #error| ::aip::FieldError::new(#at, error);
            let parsed = #name_path::parse(#value).map_err(fail)?;
            #validated
            #owned
            ::core::result::Result::Ok(parsed)
        }
    } else {
        quote! {
            #name_path::parse(#value).map_err(|error| ::aip::FieldError::new(#at, error))
        }
    };

    let held = if validate {
        " and validates it, so a wildcard is no name"
    } else if own.is_some() {
        ". Its parents may be the wildcard, but not the resource itself"
    } else {
        ""
    };
    let method_doc = doc(&format!(
        "Parses `{}` as the `{}` resource name it references{held}.\n\n\
         # Errors\n\n\
         If the field does not hold a name of that resource; the \
         [`FieldError`](::aip::FieldError) names `{}` for it.",
        reference.field_name, resource.resource_type, reference.field_name,
    ));
    impl_for(
        &message,
        options.views,
        &quote! {
            #method_doc
            pub fn #method(
                &self,
            ) -> ::core::result::Result<#name_path, ::aip::FieldError> {
                #body
            }
        },
    )
}

/// The `aip::FieldPath` to the `string` field `name`, numbered `number`: where a
/// `FieldError` from parsing it says the failure is.
fn field_at(name: &str, number: u32) -> TokenStream {
    let name = Literal::string(name);
    let number = Literal::u32_unsuffixed(number);
    quote! { ::aip::FieldPath::field(#name, #number, ::aip::FieldType::String) }
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
                .and_then(|parent| {
                    registry
                        .find_by_pattern(&parent, &resource.package)
                        .map(|(r, _)| r)
                })
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

/// Emits the AIP-133 create-ID accessor for every resource whose create
/// request is declared in `file`.
///
/// AIP-133 gives a create request a `{resource}_id` the caller may propose and
/// may leave empty, in which case the *server* assigns one. That the ID is a
/// UUID is a schema fact — `google.api.field_info.format = UUID4`, the same
/// annotation that types the segment on the name — but that an empty one means
/// "mint one" is an AIP fact, and it is the same on every such request. Every
/// consumer was writing it out by hand.
///
/// Only for a resource with one pattern and a UUID-typed own ID. A `String` ID
/// has no minting rule the schema states — what a server generates is its own
/// business — and a multi-pattern resource's create request does not say which
/// pattern it is creating under, so neither gets one rather than getting a
/// guess.
#[must_use]
pub fn emit_create_ids(file: &str, index: &Index, registry: &Registry, views: bool) -> TokenStream {
    let impls: Vec<TokenStream> = registry
        .resources
        .iter()
        .filter_map(|resource| emit_create_id(file, index, resource, views))
        .collect();
    quote! { #( #impls )* }
}

fn emit_create_id(
    file: &str,
    index: &Index,
    resource: &Resource,
    views: bool,
) -> Option<TokenStream> {
    if resource.is_multi_pattern() {
        return None;
    }
    // The resource's own ID is the last variable segment of its only pattern;
    // the ones before it belong to its parents, which their own create requests
    // mint.
    let segment = resource
        .patterns
        .first()?
        .segments
        .iter()
        .rev()
        .find(|segment| segment.variable)?;
    if segment.format != Format::Uuid {
        return None;
    }

    // AIP-133 names the request after the resource, which is also how the
    // format was found in the first place -- see `scan::create_requests`.
    // Nested or not -- protoc-gen-go-aip finds either -- but in the resource's
    // own package and in this file.
    let request_name = format!("Create{}Request", resource.type_name);
    let request = index.in_file(file).find(|message| {
        message.package == resource.package && short_name(&message.fqn) == request_name
    })?;
    let field_name = format!("{}_id", snake_case(&segment.name));
    let field = request
        .fields
        .iter()
        .find(|field| field.name == field_name && field.kind == Kind::String)?;

    let path: TokenStream = request
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let ident = buffa_codegen::idents::make_field_ident(&field.name);
    let method = format_ident!("parse_{}", field.name);
    // An editions or proto2 field is an `Option<String>`; read as `""` when
    // unset, which AIP-133 reads as "the server assigns one" either way.
    let value = if field.optional {
        quote! { self.#ident.as_deref().unwrap_or_default() }
    } else {
        quote! { &self.#ident }
    };
    let name_error = name_error();
    let field_name = Literal::string(&field.name);
    let at = field_at(&field.name, u32::try_from(field.number).unwrap_or_default());

    let method_doc = doc(&format!(
        "The ID the caller proposed for the `{}` being created, or `None` when \
         they left `{}` empty.\n\n\
         AIP-133: an empty `{}` means the server assigns one, and how is the \
         server's to decide -- a version 7 UUID for an index that stays in \
         insertion order, a version 4 for one that reveals nothing -- so it is \
         handed back as \
         `None` for the call site to fill: `.unwrap_or_else(Uuid::now_v7)`. \
         The field holds a bare ID rather than a resource name, so there is no \
         pattern to scan -- build the name from the result with `{} {{ {} }}`.\n\n\
         Emitted because the schema types this ID as a UUID, through \
         `google.api.field_info`. A `string` ID gets no accessor: what a valid \
         one is, the schema does not state.\n\n\
         # Errors\n\n\
         If the caller proposed an ID that is not a canonical lowercase UUID, \
         or is the nil UUID, which no `{}` validates with. Reported as a \
         [`ResourceNameError`](::aip::resource::ResourceNameError) naming the \
         field -- `parse {} \"abc\": ...`, as aip-go's accessor reports it -- in a \
         [`FieldError`](::aip::FieldError) that names it too, so a call site \
         handles one error type for an id and a name alike.",
        resource.resource_type,
        field.name,
        field.name,
        resource.name_type(),
        buffa_codegen::idents::make_field_ident(&format!("{}_id", snake_case(&segment.name))),
        resource.name_type(),
        field.name,
    ));

    Some(impl_for(
        &path,
        views,
        &quote! {
            #method_doc
            pub fn #method(
                &self,
            ) -> ::core::result::Result<
                ::core::option::Option<::uuid::Uuid>,
                ::aip::FieldError,
            > {
                let id: &str = #value;
                if id.is_empty() {
                    return ::core::result::Result::Ok(::core::option::Option::None);
                }
                let fail = |reason: &dyn ::core::fmt::Display| {
                    ::aip::FieldError::new(
                        #at,
                        #name_error::new(::std::format!("parse {} {id:?}: {reason}", #field_name)),
                    )
                };
                match <::uuid::Uuid as ::core::str::FromStr>::from_str(id) {
                    ::core::result::Result::Ok(value) => {
                        // As a name's segment: the form it formats back to, only.
                        if ::std::string::ToString::to_string(&value) != id {
                            return ::core::result::Result::Err(fail(&"not a canonical lowercase UUID"));
                        }
                        // The nil UUID parses, but no name built from it validates.
                        if value.is_nil() {
                            return ::core::result::Result::Err(fail(&"the nil UUID is not an ID"));
                        }
                        ::core::result::Result::Ok(::core::option::Option::Some(value))
                    }
                    ::core::result::Result::Err(error) => {
                        ::core::result::Result::Err(fail(&error))
                    }
                }
            }
        },
    ))
}
