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

// --- implied_update_mask ----------------------------------------------------

use aip_fixture::proto::example::v1::{Address, Carrier, Meter, Parcel, Unit, meter};

#[test]
fn an_empty_resource_implies_an_empty_mask() {
    assert!(Shipment::default().implied_update_mask().is_empty());
    assert!(Meter::default().implied_update_mask().is_empty());
}

#[test]
fn implies_every_populated_writable_field_in_declaration_order() {
    let shipment = Shipment {
        reference: "r1".to_owned(),
        carrier: Carrier::default().into(),
        parcels: vec![Parcel::default()],
        parcels_by_code: [("p1".to_owned(), Parcel::default())].into_iter().collect(),
        origin: Address::default().into(),
        ..Default::default()
    };
    assert_eq!(
        shipment.implied_update_mask(),
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
fn never_implies_a_field_an_update_may_not_write() {
    // Every unwritable field set, nothing writable: OUTPUT_ONLY, IDENTIFIER and
    // IMMUTABLE are left out however they are populated.
    let shipment = Shipment {
        name: "shipments/s1".to_owned(),
        tracking_id: "t1".to_owned(),
        audit_log: vec!["created".to_owned()],
        revision: 3,
        etag: "e1".to_owned(),
        ..Default::default()
    };
    assert!(shipment.implied_update_mask().is_empty());

    let meter = Meter {
        serial: "s1".to_owned(),
        ..Default::default()
    };
    assert!(meter.implied_update_mask().is_empty());
}

#[test]
fn reads_presence_as_protobuf_does() {
    let meter = Meter {
        // Explicit presence: set to zero still counts.
        threshold: Some(0),
        // A oneof member counts when it is the one set, even to zero.
        reading: Some(meter::Reading::Count(0)),
        ..Default::default()
    };
    assert_eq!(meter.implied_update_mask(), ["threshold", "count"]);

    let meter = Meter {
        // Implicit presence: populated when not the zero value.
        level: 0.5,
        unit: Unit::UNIT_LITRE.into(),
        active: true,
        calibration: vec![1],
        reading: Some(meter::Reading::Label(String::new())),
        ..Default::default()
    };
    assert_eq!(
        meter.implied_update_mask(),
        ["level", "unit", "active", "calibration", "label"]
    );
}

#[test]
fn negative_zero_is_populated() {
    // protobuf-go reads a float's presence by bit pattern, and so does this.
    let meter = Meter {
        level: -0.0,
        ..Default::default()
    };
    assert_eq!(meter.implied_update_mask(), ["level"]);
}

// --- immutable_changes ------------------------------------------------------

#[test]
fn a_changed_immutable_field_is_reported() {
    let stored = Meter {
        serial: "s1".to_owned(),
        ..Default::default()
    };
    let update = Meter {
        serial: "s2".to_owned(),
        ..Default::default()
    };
    assert_eq!(update.immutable_changes(&stored), ["serial"]);
}

#[test]
fn an_echoed_or_unset_immutable_field_is_not() {
    let stored = Meter {
        serial: "s1".to_owned(),
        ..Default::default()
    };
    // Echoed back unchanged.
    assert!(stored.clone().immutable_changes(&stored).is_empty());
    // Left unset by the update.
    assert!(Meter::default().immutable_changes(&stored).is_empty());
}

#[test]
fn an_output_only_immutable_field_is_left_to_clearing() {
    // Shipment.etag is OUTPUT_ONLY and IMMUTABLE: clear_output_only drops it,
    // so a differing value is not a change the client made.
    let update = Shipment {
        etag: "e2".to_owned(),
        ..Default::default()
    };
    let stored = Shipment {
        etag: "e1".to_owned(),
        ..Default::default()
    };
    assert!(update.immutable_changes(&stored).is_empty());
}
