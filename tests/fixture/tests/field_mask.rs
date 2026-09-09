//! Exercises `MUTABLE_PATHS` — the AIP-134 expansion of an empty `update_mask`.
//!
//! Checking that a mask names only these is *not* generated: that is
//! `(buf.validate.field).field_mask.in`, which protovalidate runs. See the
//! module docs on `emit::field_mask`.

use aip_fixture::proto::example::v1::{Book, Shipment};

#[test]
fn writable_is_every_field_no_behavior_takes_away() {
    // `reference` is unannotated; the message fields are writable because the
    // *message* is not output-only, whatever is inside it.
    assert_eq!(
        Shipment::MUTABLE_PATHS,
        [
            "reference",
            "carrier",
            "parcels",
            "parcels_by_code",
            "origin"
        ]
    );
}

#[test]
fn output_only_identifier_and_immutable_are_all_unwritable() {
    for path in [
        "tracking_id", // OUTPUT_ONLY
        "create_time", //
        "audit_log",   //
        "revision",    //
        "etag",        // OUTPUT_ONLY *and* IMMUTABLE
        "name",        // IDENTIFIER: selects the target, is not part of it
    ] {
        assert!(!Shipment::MUTABLE_PATHS.contains(&path), "{path}");
    }
}

#[test]
fn a_resource_annotating_nothing_gets_every_field() {
    // Book declares a resource but no field_behavior at all, so nothing is
    // taken away -- including `name`, which is only unwritable when the schema
    // says IDENTIFIER. This constant reports what the schema states, not what
    // AIP-122 implies about a field called `name`.
    assert_eq!(Book::MUTABLE_PATHS, ["name"]);
}

// Carrier is deliberately absent: it declares no `google.api.resource`, so it
// gets no list. A mask reaching `carrier.name` is checked by protovalidate
// against the `field_mask.in` on the mask field, which needs nothing here.
