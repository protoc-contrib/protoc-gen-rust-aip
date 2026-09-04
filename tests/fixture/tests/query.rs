//! Exercises the List query pass.

use aip_fixture::proto::example::v1::{ListVolumesRequest, SearchVolumesRequest};

fn request(filter: &str, order_by: &str) -> ListVolumesRequest {
    ListVolumesRequest {
        filter: filter.to_owned(),
        order_by: order_by.to_owned(),
        ..Default::default()
    }
}

#[test]
fn declares_every_resource_field_with_a_cel_type() {
    assert_eq!(
        ListVolumesRequest::QUERY_FIELDS,
        [
            "name",
            "title",
            "read_count",
            "published",
            "genre",
            "create_time",
            "read_time",
        ]
    );
}

#[test]
fn skips_fields_with_no_total_order_rather_than_rejecting_them() {
    // `cover` is a nested message, `tags` is repeated, `labels` is a map.
    // Volume declares all three and still generates; they are simply absent.
    for skipped in ["cover", "tags", "labels"] {
        assert!(
            !ListVolumesRequest::QUERY_FIELDS.contains(&skipped),
            "{skipped} has no CEL type and must not be declared"
        );
    }
}

#[test]
fn compiles_a_filter_over_declared_fields() {
    let request = request(r#"title == "demo" && read_count > 3"#, "");
    let program = request.parse_filter().unwrap().expect("a filter was given");
    assert!(program.references().has_variable("title"));
}

#[test]
fn an_empty_filter_is_absent_rather_than_an_error() {
    assert!(request("", "").parse_filter().unwrap().is_none());
}

#[test]
fn rejects_a_filter_naming_something_undeclared() {
    let error = request("shoe_size == 9", "").parse_filter().unwrap_err();
    match error {
        aip::query::FilterError::Undeclared { name, declared } => {
            assert_eq!(name, "shoe_size");
            // The error says what was available, since there is no type
            // checker to explain anything more.
            assert!(declared.contains(&"title".to_owned()));
        }
        other => panic!("expected an undeclared-field error, got {other:?}"),
    }
}

#[test]
fn rejects_the_aip_160_grammar_as_a_syntax_error() {
    // AIP-160 spells equality `=` and conjunction `AND`. Filters here are
    // plain CEL, so the old grammar fails loudly instead of being misread.
    let error = request(r#"title = "demo" AND published"#, "")
        .parse_filter()
        .unwrap_err();
    assert!(matches!(error, aip::query::FilterError::Syntax { .. }));
}

#[test]
fn an_enum_compares_as_an_integer() {
    // CEL models a protobuf enum as an int, which is also how a column stores
    // it: `genre == 1`, not `genre == "GENRE_FICTION"`.
    assert!(request("genre == 1", "").parse_filter().is_ok());
}

#[test]
fn parses_an_order_by_over_declared_fields() {
    let order_by = request("", "title desc, create_time")
        .parse_order_by()
        .unwrap();
    assert_eq!(
        order_by.paths().collect::<Vec<_>>(),
        ["title", "create_time"]
    );
}

#[test]
fn an_empty_order_by_is_the_servers_choice() {
    assert!(request("", "").parse_order_by().unwrap().is_empty());
}

#[test]
fn rejects_an_order_by_naming_something_undeclared() {
    let error = request("", "cover").parse_order_by().unwrap_err();
    assert!(matches!(error, aip::QueryError::NotOrderable(_)));
}

#[test]
fn an_empty_page_token_is_the_first_page() {
    let request = request("", "");
    let token = request.parse_page_token().unwrap();
    assert_eq!(token.offset, 0);
    assert_eq!(token.request_checksum, request.checksum());
}

#[test]
fn a_token_round_trips_within_one_query() {
    let request = request("published", "title");
    let token = request.parse_page_token().unwrap();
    let next = token.next_offset(25);

    let page_two = ListVolumesRequest {
        page_token: next.encode(),
        ..request
    };
    assert_eq!(page_two.parse_page_token().unwrap().offset, 25);
}

#[test]
fn a_token_is_rejected_once_the_query_changes() {
    let first = request("published", "title");
    let token = first.parse_page_token().unwrap().next_offset(25);

    // Same token, different filter: the checksum no longer matches, which is
    // what stops a client paging through a query it has silently altered.
    let changed = ListVolumesRequest {
        filter: "read_count > 3".to_owned(),
        page_token: token.encode(),
        ..first
    };
    assert!(matches!(
        changed.parse_page_token(),
        Err(aip::pagination::ParseError::ChecksumMismatch { .. })
    ));
}

#[test]
fn page_size_does_not_take_part_in_the_checksum() {
    // page_size is expected to change between pages, so a token issued at one
    // size must still validate at another.
    let mut request = request("published", "");
    request.page_size = 10;
    let checksum = request.checksum();
    request.page_size = 50;
    assert_eq!(request.checksum(), checksum);
}

#[test]
fn parse_query_gathers_every_dimension_the_request_carries() {
    let request = request("published", "title desc");
    let query = request.parse_query().unwrap();
    assert!(query.filter.is_some());
    assert_eq!(query.order_by.paths().collect::<Vec<_>>(), ["title"]);
    assert_eq!(query.page_token.offset, 0);
}

#[test]
fn parse_query_returns_the_first_failing_dimension() {
    let error = request("shoe_size == 9", "title")
        .parse_query()
        .unwrap_err();
    assert!(matches!(error, aip::QueryError::Filter(_)));
}

#[test]
fn a_request_gets_only_the_dimensions_it_declares() {
    // SearchVolumesRequest has `filter` and nothing else, so its query struct
    // has one field -- this would not compile if the others were emitted.
    let query = SearchVolumesRequest {
        filter: "published".to_owned(),
        ..Default::default()
    }
    .parse_query()
    .unwrap();
    assert!(query.filter.is_some());
}
