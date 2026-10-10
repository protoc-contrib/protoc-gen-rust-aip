//! Exercises the AIP-160 filter pass: `LIST_VOLUMES_FILTER_ENV` and
//! `parse_filter`.
//!
//! `order_by` and `page_token` are *not* generated: how a request sorts and
//! where a page resumes are decided where the query runs. See the module docs
//! on `emit::filter`.

use std::sync::Arc;

use aip_fixture::aip_gen::example::v1::LIST_VOLUMES_FILTER_ENV;
use aip_fixture::proto::example::v1::ListVolumesRequest;
use cel::common::ast::Expr;
use cel::{Context, Value};

fn request(filter: &str) -> ListVolumesRequest {
    ListVolumesRequest {
        filter: filter.to_owned(),
        ..Default::default()
    }
}

/// Every field of `Volume` with a CEL type, one value each: unsigned widths
/// and enums are `int`, as in Go.
const EVERY_FIELD: &str = r#"name: "volumes/v1", title: "t", read_count: 1, shelf: 2,
    rating: 4.5, published: true, genre: 1,
    create_time: timestamp("2026-01-01T00:00:00Z"), read_time: duration("90s")"#;

/// Builds a `Volume` in CEL from `fields`, in the generated environment.
fn construct(fields: &str) -> Result<Value, cel::ExecutionError> {
    let program = LIST_VOLUMES_FILTER_ENV
        .compile(&format!("example.v1.Volume{{{fields}}}"))
        .expect("a struct literal parses");
    program.execute(&Context::with_env(Arc::clone(&LIST_VOLUMES_FILTER_ENV)))
}

#[test]
fn parses_a_filter() {
    let program = request(r#"title == "demo" && read_count > 3"#)
        .parse_filter()
        .unwrap()
        .expect("a filter was given");
    let references = program.references();
    assert!(references.has_variable("title"));
    assert!(references.has_variable("read_count"));
}

#[test]
fn an_empty_filter_is_absent_rather_than_an_error() {
    assert!(request("").parse_filter().unwrap().is_none());
    // AIP-160 reads a blank filter as no filter, whitespace included.
    assert!(request("  \n").parse_filter().unwrap().is_none());
}

#[test]
fn rejects_the_aip_160_grammar_as_a_syntax_error() {
    // AIP-160 spells equality `=` and conjunction `AND`. Filters here are
    // plain CEL, so the old grammar fails loudly instead of being misread.
    assert!(
        request(r#"title = "demo" AND published"#)
            .parse_filter()
            .is_err()
    );
}

#[test]
fn rejects_optional_syntax() {
    assert!(request("cover.?url").parse_filter().is_err());
}

#[test]
fn leaves_a_macro_a_plain_call() {
    // With macros, `exists` would expand to a comprehension, which no query
    // has a reading for. Without, it is a call a query layer can refuse.
    let program = request(r#"tags.exists(t, t == "x")"#)
        .parse_filter()
        .unwrap()
        .unwrap();
    assert!(matches!(&program.expression().expr, Expr::Call(call) if call.func_name == "exists"));
}

#[test]
fn checks_syntax_only() {
    // cel-rust has no type checker and no variable declarations: a wrong type
    // and an unknown name both compile, and are the query layer's to refuse.
    assert!(request("title > 3").parse_filter().is_ok());
    assert!(request("shoe_size == 9").parse_filter().is_ok());
}

#[test]
fn a_filter_evaluates_in_the_environment() {
    let program = request(r#"title == "demo" && read_count > 3"#)
        .parse_filter()
        .unwrap()
        .unwrap();
    let mut context = Context::with_env(Arc::clone(&LIST_VOLUMES_FILTER_ENV));
    context.add_variable_from_value("title", "demo");
    context.add_variable_from_value("read_count", 7i64);
    assert_eq!(program.execute(&context), Ok(Value::Bool(true)));
}

#[test]
fn registers_the_resource_with_every_field_that_has_a_cel_type() {
    let value = construct(EVERY_FIELD);
    assert!(value.is_ok(), "{value:?}");
}

#[test]
fn a_registered_field_is_typed() {
    let fields = EVERY_FIELD.replace(r#"title: "t""#, "title: 3");
    assert!(matches!(
        construct(&fields),
        Err(cel::ExecutionError::UnexpectedType { .. })
    ));
}

#[test]
fn skips_fields_with_no_cel_type() {
    // Declared on the message, but `bytes`, a nested message, a repeated field
    // and a map have no CEL type, so the struct does not have them.
    for extra in [r#"checksum: b"x""#, "cover: 1", "tags: []", "labels: {}"] {
        assert!(
            matches!(
                construct(&format!("{EVERY_FIELD}, {extra}")),
                Err(cel::ExecutionError::NoSuchKey(_))
            ),
            "{extra} must not be registered"
        );
    }
}
