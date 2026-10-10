//! Emits the AIP-160 `filter` environment for a `List` request, and a
//! `parse_filter` that compiles in it.
//!
//! Nothing is annotated. A request is a `List` request when a service method
//! takes it and returns a message with a single repeated message field; that
//! field's type is the resource, and its fields are what get declared — the
//! same rule, and the same field-to-type mapping, as protoc-gen-go-aip's
//! `FilterEnv`.
//!
//! `order_by` and `page_token` get nothing: how a request sorts and where a
//! page resumes are decided where the query runs.

use std::collections::BTreeMap;

use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use crate::emit::{doc, short_name};
use crate::idents::snake_case;
use crate::messages::{Field, Index, Kind, Message};

/// A `List` request and the resource it lists.
pub struct ListRequest {
    /// The request message.
    pub request: String,
    /// The resource message, read off the response's repeated field.
    pub resource: String,
}

/// Finds every `List` request with a `filter` in the schema, keyed by request
/// message.
///
/// Keyed rather than listed because two methods may take the same request —
/// emitting its environment twice would be a duplicate-definition error.
#[must_use]
pub fn plan(index: &Index) -> BTreeMap<String, ListRequest> {
    let mut found = BTreeMap::new();
    for method in &index.methods {
        let Some(response) = index.get(&method.output) else {
            continue;
        };
        let Some(resource) = listed_resource(response, index) else {
            continue;
        };
        let Some(request) = index.get(&method.input) else {
            continue;
        };
        if !has_filter(request) {
            continue;
        }
        found.insert(
            method.input.clone(),
            ListRequest {
                request: method.input.clone(),
                resource,
            },
        );
    }
    found
}

/// The resource a response lists: the type of its single repeated message
/// field.
///
/// More than one, or none, means this is not a `List` response — a response
/// with two repeated message fields does not say which one is the resource,
/// and guessing would declare whichever was declared first.
fn listed_resource(response: &Message, index: &Index) -> Option<String> {
    let mut repeated = response
        .fields
        .iter()
        .filter(|field| field.repeated && !field.is_map && field.kind == Kind::Message);
    let first = repeated.next()?;
    if repeated.next().is_some() {
        return None;
    }
    // Only a resource this generator can see the fields of is usable.
    index.get(&first.type_name)?;
    Some(first.type_name.clone())
}

/// Whether a request carries the AIP-mandated `string filter` field.
fn has_filter(request: &Message) -> bool {
    request
        .fields
        .iter()
        .any(|field| field.name == "filter" && field.kind == Kind::String && !field.repeated)
}

/// The `cel::common::types` constant a field is declared with, or `None` if it
/// has no CEL type.
///
/// Go's `celType`, case for case: every integer width and signedness is `int`,
/// as is an enum; `bytes`, a nested message other than `Timestamp` or
/// `Duration`, a repeated field and a map have none, and are **skipped, not
/// rejected** — the schema is not at fault for containing one.
fn cel_type(field: &Field) -> Option<&'static str> {
    if field.repeated || field.is_map {
        return None;
    }
    match field.kind {
        Kind::String => Some("STRING_TYPE"),
        Kind::Bool => Some("BOOL_TYPE"),
        Kind::Integer | Kind::Enum => Some("INT_TYPE"),
        Kind::Double => Some("DOUBLE_TYPE"),
        Kind::Bytes => None,
        Kind::Message => match field.type_name.as_str() {
            ".google.protobuf.Timestamp" => Some("TIMESTAMP_TYPE"),
            ".google.protobuf.Duration" => Some("DURATION_TYPE"),
            _ => None,
        },
    }
}

/// The name of a request's environment: `ListVolumesRequest` gives
/// `LIST_VOLUMES_FILTER_ENV`, as Go's gives `ListVolumesFilterEnv`.
///
/// Statics share their package's module, so a nested request keeps its
/// parents' names — `Outer.ListThingsRequest` gives
/// `OUTER_LIST_THINGS_FILTER_ENV` — rather than colliding with a top-level one.
fn env_name(fqn: &str, package: &str) -> String {
    let fqn = fqn.trim_start_matches('.');
    let relative = fqn
        .strip_prefix(package)
        .map_or(fqn, |rest| rest.trim_start_matches('.'));
    let path = relative.strip_suffix("Request").unwrap_or(relative);
    let words: Vec<String> = path.split('.').map(snake_case).collect();
    format!("{}_FILTER_ENV", words.join("_").to_uppercase())
}

/// Emits the filter environment for every `List` request declared in one file.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    requests: &BTreeMap<String, ListRequest>,
) -> TokenStream {
    let items: Vec<TokenStream> = index
        .in_file(file)
        .filter_map(|message| requests.get(&message.fqn))
        .filter_map(|list| emit_request(list, index))
        .collect();
    quote! { #( #items )* }
}

fn emit_request(list: &ListRequest, index: &Index) -> Option<TokenStream> {
    let request = index.get(&list.request)?;
    let resource = index.get(&list.resource)?;
    let resource_short = short_name(&resource.fqn);
    let request_short = short_name(&request.fqn);

    let path: TokenStream = request
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");
    let env = Ident::new(&env_name(&request.fqn, &request.package), Span::call_site());
    let struct_name = Literal::string(resource.fqn.trim_start_matches('.'));

    let fields: Vec<TokenStream> = resource
        .fields
        .iter()
        .filter_map(|field| {
            let ty = Ident::new(cel_type(field)?, Span::call_site());
            let name = Literal::string(&field.name);
            Some(quote! {
                .add_field(::std::borrow::ToOwned::to_owned(#name), ::cel::common::types::#ty)
            })
        })
        .collect();

    let env_doc = doc(&format!(
        "The CEL environment `filter` expressions on `{request_short}` compile \
         in — protoc-gen-go-aip's `FilterEnv`.\n\n\
         The standard library without macros or optional syntax — `exists`, \
         `has` and the like stay plain calls, and `a.?b` does not parse, since \
         neither has a reading as a query — and `{resource_short}` registered as \
         a struct type with every field that has a CEL type. cel-rust has no \
         type checker and \
         no variable declarations, so a registered field constrains a \
         `{resource_short}{{...}}` literal, not the names a filter may use: \
         `title > 3` compiles here, unlike in Go. Checking names and types, \
         and which fields a client may *actually* filter by, is the query \
         layer's.",
    ));
    let method_doc = doc(&format!(
        "Compiles the AIP-160 `filter` expression in \
         [`{env}`], returning `None` when the request carries none.\n\n\
         The expression is CEL, not the AIP-160 grammar: `=`, uppercase \
         `AND`/`OR`/`NOT` and `:` are rejected as syntax errors rather than \
         silently misread. Nothing beyond syntax is checked — see [`{env}`].\n\n\
         # Errors\n\n\
         If the expression is not valid CEL.",
    ));

    Some(quote! {
        #env_doc
        pub static #env: ::std::sync::LazyLock<::std::sync::Arc<::cel::Env>> =
            ::std::sync::LazyLock::new(|| {
                // Optional support is read by `with_stdlib`, so it is turned
                // off first.
                let mut env = ::cel::Env::default()
                    .with_optional_support(false)
                    .with_stdlib();
                env.add_type(
                    ::cel::StructDef::new(::std::borrow::ToOwned::to_owned(#struct_name))
                        #( #fields )*,
                )
                .expect("a message name is a valid CEL type name, registered once");
                ::std::sync::Arc::new(env)
            });

        impl #path {
            #method_doc
            pub fn parse_filter(
                &self,
            ) -> ::core::result::Result<::core::option::Option<::cel::Program>, ::cel::ParseErrors>
            {
                if self.filter.trim().is_empty() {
                    return ::core::result::Result::Ok(::core::option::Option::None);
                }
                #env.compile(&self.filter).map(::core::option::Option::Some)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_env_after_the_request_less_its_suffix() {
        assert_eq!(
            env_name(".example.v1.ListVolumesRequest", "example.v1"),
            "LIST_VOLUMES_FILTER_ENV"
        );
        // A request that does not end in `Request` keeps its whole name.
        assert_eq!(
            env_name(".example.v1.SearchVolumes", "example.v1"),
            "SEARCH_VOLUMES_FILTER_ENV"
        );
    }

    #[test]
    fn a_nested_request_keeps_its_parents_name() {
        assert_eq!(
            env_name(".example.v1.Outer.ListThingsRequest", "example.v1"),
            "OUTER_LIST_THINGS_FILTER_ENV"
        );
    }
}
