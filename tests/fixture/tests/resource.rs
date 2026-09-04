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

#[test]
fn parsing_rejects_a_name_of_another_shape() {
    assert!(ex::PublisherName::parse("publishers/p1/books/b1").is_err());
    assert!(ex::PublisherName::parse("authors/a1").is_err());
    assert!(ex::PublisherName::parse("publishers/").is_err());
}

#[test]
fn the_fully_qualified_form_requires_its_own_domain() {
    assert!(ex::PublisherName::parse_full("//example.com/publishers/p1").is_ok());
    // The right shape under the wrong service is not this resource.
    assert!(ex::PublisherName::parse_full("//other.example/publishers/p1").is_err());
    // Nor is the relative form, which `parse` is for.
    assert!(ex::PublisherName::parse_full("publishers/p1").is_err());
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
    let error = ex::BookName::parse("shelves/s1/books/b1").unwrap_err();
    assert_eq!(error.name(), "shelves/s1/books/b1");
    assert_eq!(error.attempts().len(), 2);
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
    assert_eq!(error.segment(), Some("publisher"));
}

// --- UUID-typed segments (google.api.field_info.format = UUID4) ------------

/// A valid version 4 UUID, used wherever a test needs a concrete id.
const ID: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

#[test]
fn an_annotated_id_becomes_a_uuid_field() {
    let name = ex::CollectionName::parse(&format!("collections/{ID}")).unwrap();
    // The field is a Uuid, not a String -- this would not compile otherwise.
    let id: uuid::Uuid = name.collection_id;
    assert_eq!(id, uuid::Uuid::parse_str(ID).unwrap());
    assert_eq!(name.to_string(), format!("collections/{ID}"));
}

#[test]
fn a_segment_that_is_not_a_uuid_fails_to_parse() {
    let error = ex::CollectionName::parse("collections/not-a-uuid").unwrap_err();
    assert!(matches!(
        error.kind(),
        aip::resource::ScanErrorKind::InvalidValue { name, .. } if name == "collection"
    ));
    // It is reported as the name not matching the pattern, not as a separate
    // error type the caller has to handle alongside ScanError.
    assert!(
        error
            .to_string()
            .contains("invalid value for segment (collection)")
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
    // Typed straight through, with no string in between.
    assert_eq!(parent.organization_id, item.organization_id);
    assert_eq!(parent.item_name(item.item_id), item);
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
            organization_id: uuid::Uuid::parse_str(ID).unwrap()
        }
        .note_name("n1"),
        note
    );
}

#[test]
fn only_an_untyped_segment_can_hold_the_wildcard() {
    // The string half can be the wildcard...
    let note = ex::NoteName::parse(&format!("organizations/{ID}/notes/-")).unwrap();
    assert!(note.contains_wildcard());
    // ...and the UUID half cannot, since "-" is not a UUID.
    assert!(ex::NoteName::parse("organizations/-/notes/n1").is_err());
    // A name with no string segment at all can never carry one.
    let item = ex::ItemName::parse(&format!("organizations/{ID}/items/{ID}")).unwrap();
    assert!(!item.contains_wildcard());
}

#[test]
fn the_nil_uuid_is_rejected_as_a_degenerate_id() {
    let name = ex::CollectionName {
        collection_id: uuid::Uuid::nil(),
    };
    let error = name.validate().unwrap_err();
    assert_eq!(error.segment(), Some("collection"));
    assert!(
        ex::CollectionName::parse(&format!("collections/{ID}"))
            .unwrap()
            .validate()
            .is_ok()
    );
}
