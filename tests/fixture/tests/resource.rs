//! Exercises the resource pass against the code it actually generates.
//!
//! The fixture crate compiling at all is already half the test — it proves the
//! emitted Rust type-checks against real buffa messages. These check the other
//! half: that it does the right thing.

use aip::ResourceName;
use aip_fixture::aip_gen::example::v1 as ex;
use aip_fixture::aip_gen::other::v1 as ot;
use aip_fixture::proto;

#[test]
fn a_single_pattern_resource_round_trips() {
    let name = ex::PublisherName::parse("publishers/p1").unwrap();
    assert_eq!(name.publisher_id, "p1");
    assert_eq!(name.to_string(), "publishers/p1");
    assert_eq!(name.full_name(), "//example.com/publishers/p1");
    assert_eq!(ex::PublisherName::TYPE, "example.com/Publisher");
    assert_eq!(ex::PublisherName::PATTERN, "publishers/{publisher}");
    assert!(name.validate().is_ok());
    assert!(!name.contains_wildcard());
}

/// What parsing `name` as a `PublisherName` says is wrong with it.
fn refused(name: &str) -> String {
    ex::PublisherName::parse(name).unwrap_err().to_string()
}

#[test]
fn parsing_rejects_a_name_of_another_shape_in_aip_gos_words() {
    assert_eq!(
        refused("publishers/p1/books/b1"),
        r#"parse "publishers/p1/books/b1": bad number of segments, want: 2, got: 4"#
    );
    assert_eq!(
        refused("authors/a1"),
        r#"parse "authors/a1": bad segment 0, want: "publishers", got: "authors""#
    );
    assert_eq!(
        refused("publishers/"),
        r#"parse "publishers/": empty value for segment 1"#
    );
}

#[test]
fn the_fully_qualified_form_requires_its_own_domain() {
    assert!(ex::PublisherName::parse_full("//example.com/publishers/p1").is_ok());
    // The right shape under the wrong service is not this resource.
    assert!(ex::PublisherName::parse_full("//other.example/publishers/p1").is_err());
    // Nor is the relative form, which `parse` is for.
    assert!(ex::PublisherName::parse_full("publishers/p1").is_err());
    assert_eq!(
        ex::PublisherName::parse_full("//other.example/publishers/p1")
            .unwrap_err()
            .to_string(),
        r#"parse "//other.example/publishers/p1": invalid prefix, want: "//example.com/""#
    );
    // What follows the prefix is parsed as `parse` does, and fails as it does.
    assert_eq!(
        ex::PublisherName::parse_full("//example.com/authors/a1")
            .unwrap_err()
            .to_string(),
        r#"parse "authors/a1": bad segment 0, want: "publishers", got: "authors""#
    );
}

#[test]
fn a_multi_pattern_resource_tries_each_pattern_in_order() {
    let book = ex::BookName::parse("publishers/p1/books/b1").unwrap();
    assert!(matches!(book, ex::BookName::PublisherBook(_)));

    let book = ex::BookName::parse("authors/a1/books/b1").unwrap();
    assert!(matches!(book, ex::BookName::AuthorBook(_)));
    assert_eq!(book.to_string(), "authors/a1/books/b1");
    assert_eq!(book.pattern(), "authors/{author}/books/{book}");
    assert_eq!(book.resource_type(), "example.com/Book");
}

#[test]
fn a_name_matching_no_pattern_reports_every_attempt() {
    // As aip-go's, which joins what each pattern said, one to a line.
    let error = ex::BookName::parse("shelves/s1/books/b1").unwrap_err();
    assert_eq!(
        error.to_string(),
        [
            r#"parse "shelves/s1/books/b1": no pattern matches: parse "shelves/s1/books/b1": bad segment 0, want: "publishers", got: "shelves""#,
            r#"parse "shelves/s1/books/b1": bad segment 0, want: "authors", got: "shelves""#,
        ]
        .join("\n")
    );
    let error = ex::BookName::parse_full("//example.com/shelves/s1").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with(r#"parse "shelves/s1": no pattern matches"#)
    );
    assert_eq!(
        ex::BookName::parse_full("shelves/s1")
            .unwrap_err()
            .to_string(),
        r#"parse "shelves/s1": invalid prefix, want: "//example.com/""#
    );
}

#[test]
fn a_variant_converts_into_its_enum() {
    let variant = ex::PublisherBookName::parse("publishers/p1/books/b1").unwrap();
    let book: ex::BookName = variant.clone().into();
    assert_eq!(book, ex::BookName::PublisherBook(variant));
}

#[test]
fn parent_and_the_child_builder_are_inverses() {
    let book = ex::PublisherBookName::parse("publishers/p1/books/b1").unwrap();
    let publisher = book.parent();
    assert_eq!(publisher.to_string(), "publishers/p1");
    assert_eq!(publisher.publisher_book_name("b1"), book);
}

#[test]
fn a_child_resolves_a_parent_in_another_package() {
    let shelf = ot::ShelfName::parse("publishers/p1/shelves/s1").unwrap();
    assert_eq!(
        shelf.parent(),
        ex::PublisherName {
            publisher_id: "p1".to_owned()
        }
    );
    // And the builder is emitted onto that foreign parent type.
    let built = ex::PublisherName {
        publisher_id: "p1".to_owned(),
    }
    .shelf_name("s1");
    assert_eq!(built, shelf);
}

#[test]
fn a_message_parses_its_own_name_field() {
    let book = proto::example::v1::Book {
        name: "publishers/p1/books/b1".to_owned(),
        ..Default::default()
    };
    assert!(matches!(
        book.parse_name().unwrap(),
        ex::BookName::PublisherBook(_)
    ));

    // A name that does not parse says which field it was.
    let book = proto::example::v1::Book {
        name: "shelves/s1/books/b1".to_owned(),
        ..Default::default()
    };
    let error = book.parse_name().unwrap_err();
    assert_eq!(error.path().to_string(), "name");
}

#[test]
fn a_name_field_under_another_name_is_honoured() {
    let person = proto::example::v1::Person {
        person_name: "persons/x1".to_owned(),
        ..Default::default()
    };
    assert_eq!(person.parse_name().unwrap().person_id, "x1");
}

#[test]
fn a_reference_parses_the_field_it_points_at() {
    let request = proto::example::v1::CreateBookRequest {
        parent: "publishers/p1".to_owned(),
        ..Default::default()
    };
    assert_eq!(request.parse_parent().unwrap().publisher_id, "p1");

    let request = proto::example::v1::CreateBookRequest {
        parent: "shelves/s1".to_owned(),
        ..Default::default()
    };
    let error = request.parse_parent().unwrap_err();
    assert_eq!(error.path().to_string(), "parent");
}

#[test]
fn a_reference_resolves_across_packages() {
    let entry = proto::other::v1::Entry {
        book: "authors/a1/books/b1".to_owned(),
        publisher: "publishers/p1".to_owned(),
        ..Default::default()
    };
    assert!(entry.parse_book().is_ok());
    assert_eq!(entry.parse_publisher().unwrap().publisher_id, "p1");
}

#[test]
fn a_file_scope_definition_gets_a_name_type_with_no_message() {
    let region = ex::RegionName::parse("regions/r1").unwrap();
    assert_eq!(region.region_id, "r1");
    assert_eq!(region.full_name(), "//example.com/regions/r1");
}

#[test]
fn the_wildcard_is_a_legal_id_that_marks_a_collection_read() {
    let name = ex::PublisherBookName::parse("publishers/-/books/b1").unwrap();
    assert!(name.contains_wildcard());
    // A wildcard is syntactically a fine ID; only its meaning differs.
    assert!(name.validate().is_ok());
}

#[test]
fn validate_rejects_an_id_carrying_the_separator() {
    let name = ex::PublisherName {
        publisher_id: "p/1".to_owned(),
    };
    let error = name.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "publisher: contains illegal character '/'"
    );
    // And one that is empty, which `parse` refuses but a struct literal can say.
    let name = ex::PublisherName {
        publisher_id: String::new(),
    };
    assert_eq!(name.validate().unwrap_err().to_string(), "publisher: empty");
}

// --- UUID-typed segments (google.api.field_info.format = UUID4) ------------

/// A valid version 4 UUID, used wherever a test needs a concrete id.
const ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

#[test]
fn an_annotated_id_becomes_a_uuid_segment() {
    let name = ex::CollectionName::parse(&format!("collections/{ID}")).unwrap();
    // The field is a string, which a `Uuid` or the wildcard can both be; the
    // typed id is `try_collection_id`.
    assert_eq!(name.collection_id, ID);
    assert_eq!(
        name.try_collection_id().unwrap(),
        uuid::Uuid::parse_str(ID).unwrap()
    );
    assert_eq!(name.to_string(), format!("collections/{ID}"));
}

#[test]
fn a_segment_that_is_not_a_uuid_fails_to_parse() {
    // Reported as the name not matching the pattern, as aip-go's is: the
    // segment's index, then what `uuid` says.
    let error = ex::CollectionName::parse("collections/not-a-uuid").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with(r#"parse "collections/not-a-uuid": segment 1: "#),
        "{error}"
    );
}

/// `ID` as `uuid` also reads it, but not as it formats back: uppercase,
/// braced, as a URN, and without hyphens.
const NON_CANONICAL: [&str; 4] = [
    "3F2504E0-4F89-41D3-9A0C-0305E82C3301",
    "{3f2504e0-4f89-41d3-9a0c-0305e82c3301}",
    "urn:uuid:3f2504e0-4f89-41d3-9a0c-0305e82c3301",
    "3f2504e04f8941d39a0c0305e82c3301",
];

#[test]
fn a_segment_that_is_not_a_canonical_lowercase_uuid_fails_to_parse() {
    for id in NON_CANONICAL {
        let error = ex::CollectionName::parse(&format!("collections/{id}")).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(r#"parse "collections/{id}": segment 1: not a canonical lowercase UUID"#)
        );
    }
    // A child's inherited segment is held to the same form.
    assert!(
        ex::ItemName::parse(&format!("organizations/{}/items/{ID}", NON_CANONICAL[0])).is_err()
    );
}

#[test]
fn a_uuid_id_round_trips_through_display() {
    let name = ex::CollectionName::parse(&format!("collections/{ID}")).unwrap();
    assert_eq!(ex::CollectionName::parse(&name.to_string()).unwrap(), name);
}

#[test]
fn a_child_inherits_its_parents_segment_format() {
    // Item's {organization} is typed from CreateOrganizationRequest, not from
    // Item's own create request -- the parent is what mints that id.
    let item = ex::ItemName::parse(&format!("organizations/{ID}/items/{ID}")).unwrap();
    let parent: ex::OrganizationName = item.parent();
    assert_eq!(parent.organization_id, item.organization_id);
    assert_eq!(parent.item_name(item.try_item_id().unwrap()), item);
}

#[test]
fn a_pattern_can_mix_a_typed_and_an_untyped_segment() {
    // Note's {organization} is a UUID inherited from Organization; its own
    // {note} has no format annotation and stays an opaque string.
    let note = ex::NoteName::parse(&format!("organizations/{ID}/notes/n1")).unwrap();
    assert_eq!(note.note_id, "n1");
    assert_eq!(note.parent().organization_id, note.organization_id);
    assert_eq!(
        ex::OrganizationName {
            organization_id: ID.to_owned()
        }
        .note_name("n1"),
        note
    );
}

#[test]
fn a_segment_of_either_kind_can_hold_the_wildcard() {
    // The string half...
    let note = ex::NoteName::parse(&format!("organizations/{ID}/notes/-")).unwrap();
    assert!(note.contains_wildcard());
    // ...and the typed half, as aip-go's names hold it: it is the wildcard,
    // which is why it is a string.
    let note = ex::NoteName::parse("organizations/-/notes/n1").unwrap();
    assert!(note.contains_wildcard());
    assert_eq!(note.organization_id, "-");
    assert_eq!(note.to_string(), "organizations/-/notes/n1");
    // A name with no wildcard in it has none.
    let item = ex::ItemName::parse(&format!("organizations/{ID}/items/{ID}")).unwrap();
    assert!(!item.contains_wildcard());
}

#[test]
fn only_a_name_of_wildcards_alone_is_a_wildcard() {
    let any = ex::ItemName::parse("organizations/-/items/-").unwrap();
    assert!(any.is_wildcard() && any.contains_wildcard());

    let some = ex::ItemName::parse(&format!("organizations/-/items/{ID}")).unwrap();
    assert!(!some.is_wildcard() && some.contains_wildcard());
    // The parent carries the wildcard with it.
    assert!(some.parent().contains_wildcard());
}

#[test]
fn the_wildcard_is_no_id() {
    let item = ex::ItemName::parse(&format!("organizations/-/items/{ID}")).unwrap();

    // The id that is there, and the error for the one that is not.
    assert_eq!(
        item.try_item_id().unwrap(),
        uuid::Uuid::parse_str(ID).unwrap()
    );
    let error = item.try_organization_id().unwrap_err();
    assert_eq!(error.to_string(), "organization: the wildcard is not an id");
}

#[test]
fn a_name_with_the_wildcard_is_not_valid_to_write() {
    let item = ex::ItemName::parse(&format!("organizations/-/items/{ID}")).unwrap();

    let error = item.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "organization: not a canonical non-nil UUID"
    );
}

#[test]
fn the_nil_uuid_is_rejected_as_a_degenerate_id() {
    let name = ex::CollectionName {
        collection_id: uuid::Uuid::nil().to_string(),
    };
    let error = name.validate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "collection: not a canonical non-nil UUID"
    );
    assert!(
        ex::CollectionName::parse(&format!("collections/{ID}"))
            .unwrap()
            .validate()
            .is_ok()
    );
}

// --- AIP-133 create IDs -----------------------------------------------------

#[test]
fn a_create_request_keeps_the_id_the_caller_proposed() {
    let request = proto::example::v1::CreateCollectionRequest {
        collection_id: ID.to_owned(),
        ..Default::default()
    };
    assert_eq!(
        request.parse_collection_id().unwrap(),
        Some(uuid::Uuid::parse_str(ID).unwrap())
    );
}

#[test]
fn an_empty_create_id_is_left_to_the_server() {
    // AIP-133: empty means the server assigns one -- and which kind of UUID
    // is the server's call, so the accessor mints nothing.
    let request = proto::example::v1::CreateCollectionRequest::default();
    assert_eq!(request.parse_collection_id().unwrap(), None);
}

/// The `ResourceNameError` a `FieldError` says is wrong with its field.
fn cause(error: &aip::FieldError) -> &aip::resource::ResourceNameError {
    error.failure().downcast_ref().expect("a ResourceNameError")
}

#[test]
fn a_proposed_id_that_is_not_a_uuid_fails_naming_the_field() {
    let request = proto::example::v1::CreateCollectionRequest {
        collection_id: "not-a-uuid".to_owned(),
        ..Default::default()
    };
    let error = request.parse_collection_id().unwrap_err();
    // Where, structurally, beside the text below.
    assert_eq!(error.path().to_string(), "collection_id");
    let error = cause(&error);
    // The same error type a whole name produces, naming the request field the
    // caller sent rather than a segment of a name they did not: as aip-go's
    // `ParseCollectionID` reports it.
    assert!(
        error
            .to_string()
            .starts_with(r#"parse collection_id "not-a-uuid": "#),
        "{error}"
    );
}

#[test]
fn a_proposed_id_that_is_not_a_canonical_lowercase_uuid_is_rejected() {
    for id in NON_CANONICAL {
        let request = proto::example::v1::CreateCollectionRequest {
            collection_id: id.to_owned(),
            ..Default::default()
        };
        let error = request.parse_collection_id().unwrap_err();
        assert_eq!(error.path().to_string(), "collection_id");
        let error = cause(&error);
        assert_eq!(
            error.to_string(),
            format!("parse collection_id {id:?}: not a canonical lowercase UUID")
        );
    }
}

#[test]
fn a_proposed_nil_uuid_is_rejected() {
    // It parses as a UUID, but no name built from it would validate -- so it is
    // refused here rather than accepted and then failing on the name.
    let request = proto::example::v1::CreateCollectionRequest {
        collection_id: uuid::Uuid::nil().to_string(),
        ..Default::default()
    };
    let error = request.parse_collection_id().unwrap_err();
    assert_eq!(error.path().to_string(), "collection_id");
    let error = cause(&error);
    assert_eq!(
        error.to_string(),
        format!(
            "parse collection_id {:?}: the nil UUID is not an ID",
            uuid::Uuid::nil().to_string()
        )
    );
}

#[test]
fn a_child_gets_an_accessor_for_its_own_id_only() {
    // Item's own {item} is UUID-typed, so CreateItemRequest parses it. Its
    // {organization} comes from the parent's create request, which is what
    // types it -- there is no `parse_organization_id` here.
    let request = proto::example::v1::CreateItemRequest {
        item_id: ID.to_owned(),
        ..Default::default()
    };
    assert_eq!(
        request.parse_item_id().unwrap(),
        Some(uuid::Uuid::parse_str(ID).unwrap())
    );
}

#[test]
fn the_accessors_are_emitted_for_views_too() {
    // Same three accessors, on the borrowed form: a name field, a reference,
    // and the AIP-133 create ID.
    let book = proto::example::v1::Book {
        name: "publishers/p1/books/b1".to_owned(),
        ..Default::default()
    };
    let book = proto::example::v1::BookOwnedView::from_owned(&book).unwrap();
    assert!(book.view().parse_name().is_ok());
    let bad = proto::example::v1::Book {
        name: "shelves/s1/books/b1".to_owned(),
        ..Default::default()
    };
    let bad = proto::example::v1::BookOwnedView::from_owned(&bad).unwrap();
    assert_eq!(
        bad.view().parse_name().unwrap_err().path().to_string(),
        "name"
    );

    let request = proto::example::v1::CreateBookRequest {
        parent: "publishers/p1".to_owned(),
        ..Default::default()
    };
    let request = proto::example::v1::CreateBookRequestOwnedView::from_owned(&request).unwrap();
    assert_eq!(request.view().parse_parent().unwrap().publisher_id, "p1");
    let bad = proto::example::v1::CreateBookRequest {
        parent: "shelves/s1".to_owned(),
        ..Default::default()
    };
    let bad = proto::example::v1::CreateBookRequestOwnedView::from_owned(&bad).unwrap();
    assert_eq!(
        bad.view().parse_parent().unwrap_err().path().to_string(),
        "parent"
    );

    let create = proto::example::v1::CreateCollectionRequest {
        collection_id: ID.to_owned(),
        ..Default::default()
    };
    let create = proto::example::v1::CreateCollectionRequestOwnedView::from_owned(&create).unwrap();
    assert_eq!(
        create.view().parse_collection_id().unwrap(),
        Some(uuid::Uuid::parse_str(ID).unwrap())
    );
}

// --- The name or parent of a request ---------------------------------------

/// The failure a `FieldError` says is wrong with its field, as text.
fn why(error: &aip::FieldError) -> String {
    error.failure().to_string()
}

#[test]
fn a_request_is_strict_by_default_so_a_wildcard_is_no_name() {
    // A key a method acts on: parsed, and then validated.
    let get = proto::example::v1::GetItemRequest {
        name: format!("organizations/{ID}/items/{ID}"),
        ..Default::default()
    };
    assert_eq!(get.parse_name().unwrap().item_id, ID);

    let get = proto::example::v1::GetItemRequest {
        name: format!("organizations/-/items/{ID}"),
        ..Default::default()
    };
    let error = get.parse_name().unwrap_err();
    assert_eq!(error.path().to_string(), "name");
    assert_eq!(why(&error), "organization: not a canonical non-nil UUID");

    // A name of the wrong shape fails the parse, as before, before validating.
    let get = proto::example::v1::GetItemRequest {
        name: "items/1".to_owned(),
        ..Default::default()
    };
    assert!(why(&get.parse_name().unwrap_err()).starts_with(r#"parse "items/1": bad number"#));

    // For a resource of several patterns too, through the enum.
    let get = proto::example::v1::GetBookRequest {
        name: "publishers/p1/books/b1".to_owned(),
        ..Default::default()
    };
    assert!(get.parse_name().is_ok());
    let get = proto::example::v1::GetBookRequest {
        name: "publishers/p1/books/".to_owned(),
        ..Default::default()
    };
    assert!(get.parse_name().is_err());

    // The `parent` of a create is held to it too.
    let create = proto::example::v1::CreateItemRequest {
        parent: "organizations/-".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        why(&create.parse_parent().unwrap_err()),
        "organization: not a canonical non-nil UUID"
    );
}

#[test]
fn a_string_id_is_not_refused_the_wildcard_by_validate() {
    // As aip-go's: `validate` refuses what is no id for certain -- empty, a `/`,
    // a UUID segment that is not a canonical, non-nil one. A string segment
    // has no other form to hold it to, so `-` is only refused by a request that
    // names the resource itself and is listed, below.
    let get = proto::example::v1::GetPublisherRequest {
        name: "publishers/-".to_owned(),
        ..Default::default()
    };
    assert!(get.parse_name().unwrap().is_wildcard());
}

#[test]
fn a_listed_request_may_read_across_parents() {
    let list = proto::example::v1::ListItemsRequest {
        parent: "organizations/-".to_owned(),
        ..Default::default()
    };
    let parent = list.parse_parent().unwrap();
    assert!(parent.is_wildcard());

    // Parsed all the same: it is a wildcard or a canonical id, nothing else.
    let list = proto::example::v1::ListItemsRequest {
        parent: "organizations/nope".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        list.parse_parent().unwrap_err().path().to_string(),
        "parent"
    );
}

#[test]
fn a_listed_name_may_have_a_wildcard_parent_but_not_be_one() {
    let delete = proto::example::v1::DeleteItemRequest {
        name: format!("organizations/-/items/{ID}"),
        ..Default::default()
    };
    let name = delete.parse_name().unwrap();
    assert!(name.contains_wildcard() && !name.is_wildcard());

    // The item itself is the resource: a wildcard is none.
    let delete = proto::example::v1::DeleteItemRequest {
        name: format!("organizations/{ID}/items/-"),
        ..Default::default()
    };
    let error = delete.parse_name().unwrap_err();
    assert_eq!(error.path().to_string(), "name");
    assert_eq!(why(&error), "item: the wildcard is not an id");

    // The same for an id of no format, which is compared rather than parsed.
    let delete = proto::example::v1::DeleteNoteRequest {
        name: "organizations/-/notes/n1".to_owned(),
        ..Default::default()
    };
    assert!(delete.parse_name().is_ok());
    let delete = proto::example::v1::DeleteNoteRequest {
        name: format!("organizations/{ID}/notes/-"),
        ..Default::default()
    };
    assert_eq!(
        why(&delete.parse_name().unwrap_err()),
        "note: the wildcard is not an id"
    );

    let delete = proto::example::v1::DeletePublisherRequest {
        name: "publishers/-".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        why(&delete.parse_name().unwrap_err()),
        "publisher: the wildcard is not an id"
    );
}

#[test]
fn a_listed_name_of_several_patterns_only_parses() {
    // The parsed name is an enum with no one segment that is its own; the
    // option does not guess.
    let delete = proto::example::v1::DeleteBookRequest {
        name: "publishers/p1/books/-".to_owned(),
        ..Default::default()
    };
    assert!(delete.parse_name().unwrap().contains_wildcard());
}

#[test]
fn any_other_reference_may_carry_a_wildcard() {
    let bookmark = proto::example::v1::Bookmark {
        publisher: "publishers/-".to_owned(),
        ..Default::default()
    };
    assert!(bookmark.parse_publisher().unwrap().is_wildcard());
}

#[test]
fn a_resources_own_name_is_only_parsed() {
    // The message that is the resource -- a `Book` returned by a server, say --
    // may hold a name with a wildcard: it is parsed, not acted on.
    let book = proto::example::v1::Book {
        name: "publishers/-/books/b1".to_owned(),
        ..Default::default()
    };
    assert!(book.parse_name().unwrap().contains_wildcard());
}
