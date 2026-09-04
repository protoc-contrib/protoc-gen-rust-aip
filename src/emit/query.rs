//! Emits the AIP query helpers for a `List` request: the fields a client may
//! name, and a parser for each dimension the request carries.
//!
//! Nothing is annotated. A request is a `List` request when a service method
//! takes it and returns a message with a single repeated message field; that
//! field's type is the resource, and its fields are what get declared. An
//! allow-list in the `.proto` was tried in the Go predecessor and removed —
//! it was a second copy of a policy the query layer already enforces, free to
//! drift out of agreement with the one that actually decides.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::doc;
use crate::messages::{Field, Index, Kind, Message};

/// The well-known messages with a total order, and so a CEL type. Every other
/// message-typed field is skipped.
const ORDERED_MESSAGES: &[&str] = &[".google.protobuf.Timestamp", ".google.protobuf.Duration"];

/// The `page_token` dimension's siblings, cleared before the request is
/// checksummed because they are expected to change from page to page.
const VOLATILE_FIELDS: &[&str] = &["page_token", "page_size", "skip"];

/// A `List` request and the resource it lists.
pub struct ListRequest {
    /// The request message.
    pub request: String,
    /// The resource message, read off the response's repeated field.
    pub resource: String,
}

/// Finds every `List` request in the schema, keyed by request message.
///
/// Keyed rather than listed because two methods may take the same request —
/// emitting its helpers twice would be a duplicate-definition error.
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
        // A request carrying none of the three dimensions has nothing to parse,
        // whatever its response looks like.
        if dimensions(request).is_empty() {
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
/// and guessing would bind the query surface to whichever was declared first.
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

/// The dimensions a request carries, as `(proto field name, dimension)`.
///
/// Each is a plain `string` field with the AIP-mandated name; a request that
/// does not have one simply does not get that parser.
fn dimensions(request: &Message) -> Vec<&'static str> {
    ["filter", "order_by", "page_token"]
        .into_iter()
        .filter(|name| {
            request
                .fields
                .iter()
                .any(|field| field.name == *name && field.kind == Kind::String && !field.repeated)
        })
        .collect()
}

/// Whether a field has a CEL type, and so can be declared.
///
/// Fields with no total order — a nested message that is not a `Timestamp` or
/// `Duration`, a repeated field, a map — are **skipped, not rejected**. A field
/// that is not declared is simply undeclared; the schema is not at fault for
/// containing one.
fn is_declarable(field: &Field) -> bool {
    if field.repeated || field.is_map {
        return false;
    }
    match field.kind {
        Kind::Message => ORDERED_MESSAGES.contains(&field.type_name.as_str()),
        _ => true,
    }
}

/// Emits the query helpers for every `List` request declared in one file.
#[must_use]
pub fn emit_file(
    file: &str,
    index: &Index,
    requests: &BTreeMap<String, ListRequest>,
    generated: &BTreeSet<String>,
) -> TokenStream {
    let items: Vec<TokenStream> = index
        .in_file(file)
        .filter_map(|message| requests.get(&message.fqn))
        .filter_map(|list| emit_request(list, index, generated))
        .collect();
    quote! { #( #items )* }
}

fn emit_request(
    list: &ListRequest,
    index: &Index,
    generated: &BTreeSet<String>,
) -> Option<TokenStream> {
    let request = index.get(&list.request)?;
    let resource = index.get(&list.resource)?;

    let path: TokenStream = request
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");

    let declared: Vec<&str> = resource
        .fields
        .iter()
        .filter(|field| is_declarable(field))
        .map(|field| field.name.as_str())
        .collect();
    let declared_literals: Vec<Literal> = declared.iter().map(|f| Literal::string(f)).collect();

    let carried = dimensions(request);
    let fields_doc = doc(&format!(
        "The fields of `{}` a `filter` or `order_by` may name.\n\n\
         Every field of the resource that has a CEL type, in declaration order. \
         Which of them a client may *actually* query is decided by the AIP-path \
         to database-column map at the query layer, which is fail-closed; this \
         is only what parses.",
        short_name(&resource.fqn),
    ));

    let mut methods = Vec::new();
    if carried.contains(&"filter") {
        methods.push(emit_filter(resource));
    }
    if carried.contains(&"order_by") {
        methods.push(emit_order_by());
    }
    if carried.contains(&"page_token") {
        methods.push(emit_page_token(request));
    }

    let query = emit_query_struct(request, &carried, generated);

    Some(quote! {
        impl #path {
            #fields_doc
            pub const QUERY_FIELDS: &'static [&'static str] = &[#( #declared_literals ),*];

            #( #methods )*
        }

        #query
    })
}

fn emit_filter(resource: &Message) -> TokenStream {
    let method_doc = doc(&format!(
        "Compiles the AIP-160 `filter` expression, returning `None` when the \
         request carries none.\n\n\
         The expression is CEL, not the AIP-160 grammar: `=`, uppercase \
         `AND`/`OR`/`NOT` and `:` are rejected as syntax errors rather than \
         silently misread. Every name it references must be one of \
         [`QUERY_FIELDS`](Self::QUERY_FIELDS) — a field of `{}`.\n\n\
         That name check is all the boundary can do. cel-rust has no type \
         checker, so an expression comparing a string field to an integer \
         compiles here and fails further down, unlike the Go implementation \
         where cel-go's checker rejects it.",
        short_name(&resource.fqn),
    ));
    quote! {
        #method_doc
        pub fn parse_filter(
            &self,
        ) -> ::core::result::Result<
            ::core::option::Option<::cel::Program>,
            ::aip::query::FilterError,
        > {
            if self.filter.is_empty() {
                return ::core::result::Result::Ok(::core::option::Option::None);
            }
            let program = ::cel::Program::compile(&self.filter).map_err(|error| {
                ::aip::query::FilterError::Syntax {
                    message: ::std::string::ToString::to_string(&error),
                }
            })?;
            for name in program.references().variables() {
                if !Self::QUERY_FIELDS.contains(&name) {
                    return ::core::result::Result::Err(
                        ::aip::query::FilterError::Undeclared {
                            name: ::std::borrow::ToOwned::to_owned(name),
                            declared: Self::QUERY_FIELDS
                                .iter()
                                .copied()
                                .map(::std::borrow::ToOwned::to_owned)
                                .collect(),
                        },
                    );
                }
            }
            ::core::result::Result::Ok(::core::option::Option::Some(program))
        }
    }
}

fn emit_order_by() -> TokenStream {
    let method_doc = doc(
        "Parses the AIP-132 `order_by` string, rejecting any path outside \
         [`QUERY_FIELDS`](Self::QUERY_FIELDS).\n\n\
         An empty `order_by` parses to an empty ordering, which is the \
         server's choice of order rather than an error.",
    );
    quote! {
        #method_doc
        pub fn parse_order_by(&self) -> ::core::result::Result<::aip::OrderBy, ::aip::QueryError> {
            let order_by: ::aip::OrderBy = ::core::str::FromStr::from_str(&self.order_by)
                .map_err(::aip::QueryError::OrderBy)?;
            order_by
                .validate_for_paths(Self::QUERY_FIELDS.iter().copied())
                .map_err(::aip::QueryError::NotOrderable)?;
            ::core::result::Result::Ok(order_by)
        }
    }
}

fn emit_page_token(request: &Message) -> TokenStream {
    let clears: Vec<TokenStream> = request
        .fields
        .iter()
        .filter(|field| VOLATILE_FIELDS.contains(&field.name.as_str()))
        .map(|field| {
            let ident = buffa_codegen::idents::make_field_ident(&field.name);
            quote! { request.#ident = ::core::default::Default::default(); }
        })
        .collect();

    let has_map = request.fields.iter().any(|field| field.is_map);
    let determinism = if has_map {
        "\n\n**This request has a map field.** buffa encodes a map in \
         `HashMap` iteration order unless configured otherwise, so the \
         checksum is not stable between calls and every page token will be \
         rejected. Configure that field as a `BTreeMap` in the buffa codegen \
         before relying on pagination here."
    } else {
        ""
    };

    let checksum_doc = doc(&format!(
        "The AIP-158 checksum of this request, over every field that defines \
         the query.\n\n\
         `page_token`, `page_size` and `skip` are cleared first: they are \
         expected to change from one page to the next, so including them would \
         invalidate every token as soon as it was used. A mismatch means the \
         client changed `filter` or `order_by` mid-page.{determinism}",
    ));
    let parse_doc = doc(
        "Decodes the AIP-158 `page_token` and checks it against this \
         request.\n\n\
         An empty token — the first page — yields a zero-offset token carrying \
         this request's checksum, so the result is always safe to advance and \
         hand back to the client.",
    );

    quote! {
        #checksum_doc
        pub fn request_checksum(&self) -> u32 {
            let mut request = ::core::clone::Clone::clone(self);
            #( #clears )*
            ::aip::pagination::request_checksum(
                &::buffa::Message::encode_to_vec(&request),
            )
        }

        #parse_doc
        pub fn parse_page_token(
            &self,
        ) -> ::core::result::Result<::aip::PageToken, ::aip::pagination::ParseError> {
            ::aip::PageToken::parse(&self.page_token, self.request_checksum())
        }
    }
}

/// The struct bundling whichever dimensions the request carries, plus the
/// `parse_query` that fills it.
fn emit_query_struct(
    request: &Message,
    carried: &[&'static str],
    generated: &BTreeSet<String>,
) -> TokenStream {
    let _ = generated;
    let name = format_ident!("{}", query_type(&request.fqn));

    let mut fields = Vec::new();
    let mut parses = Vec::new();
    let mut inits = Vec::new();

    if carried.contains(&"filter") {
        let doc = doc("The compiled `filter`, or `None` if the request carried none.");
        fields.push(quote! { #doc pub filter: ::core::option::Option<::cel::Program> });
        parses.push(quote! {
            let filter = self.parse_filter().map_err(::aip::QueryError::Filter)?;
        });
        inits.push(quote! { filter });
    }
    if carried.contains(&"order_by") {
        let doc = doc("The parsed `order_by`, empty if the request carried none.");
        fields.push(quote! { #doc pub order_by: ::aip::OrderBy });
        parses.push(quote! { let order_by = self.parse_order_by()?; });
        inits.push(quote! { order_by });
    }
    if carried.contains(&"page_token") {
        let doc = doc("The decoded `page_token`, at offset zero for the first page.");
        fields.push(quote! { #doc pub page_token: ::aip::PageToken });
        parses.push(quote! {
            let page_token = self.parse_page_token().map_err(::aip::QueryError::PageToken)?;
        });
        inits.push(quote! { page_token });
    }

    let path: TokenStream = request
        .rust_path
        .parse()
        .expect("a message path built from proto identifiers is a valid Rust path");

    let struct_doc = doc(&format!(
        "The parsed query dimensions of a [`{}`].\n\n\
         One field per dimension the request declares; a dimension the request \
         does not carry is absent here rather than empty.",
        short_name(&request.fqn),
    ));
    let method_doc = doc(
        "Parses every AIP dimension this request carries, applying the same \
         checks as the per-dimension parsers.\n\n\
         The first failing dimension's error is returned; map it to \
         `InvalidArgument` at the RPC boundary.",
    );

    quote! {
        #struct_doc
        #[derive(::core::fmt::Debug)]
        pub struct #name {
            #( #fields, )*
        }

        impl #path {
            #method_doc
            pub fn parse_query(
                &self,
            ) -> ::core::result::Result<#name, ::aip::QueryError> {
                #( #parses )*
                ::core::result::Result::Ok(#name { #( #inits, )* })
            }
        }
    }
}

/// The name of a request's query struct: `ListBooksRequest` gives
/// `ListBooksQuery`.
fn query_type(fqn: &str) -> String {
    let name = short_name(fqn);
    format!("{}Query", name.strip_suffix("Request").unwrap_or(name))
}

/// The last segment of a fully-qualified proto name.
fn short_name(fqn: &str) -> &str {
    fqn.rsplit('.').next().unwrap_or(fqn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_query_struct_after_the_request() {
        assert_eq!(query_type(".example.v1.ListBooksRequest"), "ListBooksQuery");
        // A request that does not end in `Request` keeps its whole name, rather
        // than losing a suffix it never had.
        assert_eq!(query_type(".example.v1.ListBooks"), "ListBooksQuery");
    }
}
