//! Exercises two versions of one API declaring the same resource type: each
//! binds to its own declaration.

use aip_fixture::aip_gen::example::{v1, v2};
use aip_fixture::proto::example::v2::Shelf;

const ID: &str = "0190b8c6-6f6a-7d6e-9a3b-0c1d2e3f4a5b";

#[test]
fn a_reference_binds_to_its_own_versions_resource() {
    let shelf = Shelf {
        collection: "collections/c1".to_owned(),
        ..Default::default()
    };
    // v2's Collection, whose ID is a string -- v1's is a UUID, so "c1" would
    // not parse as one.
    let collection: v2::CollectionName = shelf.parse_collection().unwrap();
    assert_eq!(collection.collection_id, "c1");
}

#[test]
fn one_versions_create_request_does_not_type_the_others_segment() {
    // v1's CreateCollectionRequest types {collection} as a UUID; v2 has none.
    let v1 = v1::CollectionName::parse(&format!("collections/{ID}")).unwrap();
    assert_eq!(v1.collection_id, uuid::Uuid::parse_str(ID).unwrap());
    let v2 = v2::CollectionName::parse("collections/not-a-uuid").unwrap();
    assert_eq!(v2.collection_id, "not-a-uuid");
}
