//! Exercises the AIP-203 `OUTPUT_ONLY` clearing walk.

// Clearing sets a field to its `Default`, so `0.0` is the exact value under
// test rather than the result of any arithmetic.
#![allow(
    clippy::float_cmp,
    reason = "asserting a field was reset to its default"
)]

use aip_fixture::proto::example::v1::{Address, Carrier, Event, Parcel, Shipment, event};
use buffa::MessageField;

fn carrier(name: &str) -> Carrier {
    Carrier {
        name: name.to_owned(),
        scac: "SERVER".to_owned(),
        ..Default::default()
    }
}

fn parcel(label: &str) -> Parcel {
    Parcel {
        label: label.to_owned(),
        billed_weight: 12.5,
        carrier: MessageField::some(carrier("inner")),
        ..Default::default()
    }
}

#[test]
fn clears_output_only_fields_of_every_shape() {
    let mut shipment = Shipment {
        reference: "keep me".to_owned(),
        tracking_id: "SERVER-SET".to_owned(),
        audit_log: vec!["created".to_owned()],
        revision: 7,
        etag: "abc".to_owned(),
        ..Default::default()
    };
    shipment.clear_output_only();

    assert_eq!(shipment.reference, "keep me", "a client field must survive");
    assert_eq!(shipment.tracking_id, "");
    assert!(shipment.audit_log.is_empty());
    assert_eq!(shipment.revision, 0);
    // Carrying IMMUTABLE as well as OUTPUT_ONLY still counts as output-only.
    assert_eq!(shipment.etag, "");
}

#[test]
fn clears_an_output_only_message_field_entirely() {
    let mut shipment = Shipment {
        create_time: MessageField::some(buffa_types::google::protobuf::Timestamp {
            seconds: 1_700_000_000,
            ..Default::default()
        }),
        ..Default::default()
    };
    shipment.clear_output_only();
    assert!(shipment.create_time.is_unset());
}

#[test]
fn descends_into_a_singular_message() {
    let mut shipment = Shipment {
        carrier: MessageField::some(carrier("DHL")),
        ..Default::default()
    };
    shipment.clear_output_only();

    let carrier = shipment.carrier.as_option().expect("still set");
    assert_eq!(carrier.name, "DHL", "the message itself is not output-only");
    assert_eq!(carrier.scac, "", "but its output-only field is cleared");
}

#[test]
fn descends_into_repeated_and_map_messages() {
    let mut shipment = Shipment {
        parcels: vec![parcel("a"), parcel("b")],
        ..Default::default()
    };
    shipment
        .parcels_by_code
        .insert("code".to_owned(), parcel("c"));
    shipment.clear_output_only();

    for parcel in &shipment.parcels {
        assert_eq!(parcel.billed_weight, 0.0);
    }
    let mapped = &shipment.parcels_by_code["code"];
    assert_eq!(mapped.billed_weight, 0.0);
    assert_eq!(mapped.label, "c");
}

#[test]
fn the_walk_is_recursive_rather_than_one_deep() {
    let mut shipment = Shipment {
        parcels: vec![parcel("a")],
        ..Default::default()
    };
    shipment.clear_output_only();

    // Shipment -> Parcel -> Carrier.scac, two levels down.
    let inner = shipment.parcels[0].carrier.as_option().expect("still set");
    assert_eq!(inner.scac, "");
    assert_eq!(inner.name, "inner");
}

#[test]
fn a_clean_message_gets_no_walk_at_all() {
    // `Address` has no output-only field at any depth. If a walk had been
    // emitted for it this would still compile, so the real assertion is the
    // one below: `Shipment` must not try to descend into it.
    let address = Address {
        line1: "1 Main St".to_owned(),
        city: "Springfield".to_owned(),
        ..Default::default()
    };
    let mut shipment = Shipment {
        origin: MessageField::some(address),
        ..Default::default()
    };
    shipment.clear_output_only();
    let origin = shipment.origin.as_option().expect("untouched");
    assert_eq!(origin.line1, "1 Main St");
    assert_eq!(origin.city, "Springfield");
}

#[test]
fn clears_a_oneof_whose_active_member_is_output_only() {
    let mut event = Event {
        id: "e1".to_owned(),
        detail: Some(event::Detail::ServerTrace("internal".to_owned())),
        ..Default::default()
    };
    event.clear_output_only();
    assert!(event.detail.is_none());
    assert_eq!(event.id, "e1");
}

#[test]
fn leaves_a_oneof_whose_active_member_is_not_output_only() {
    let mut event = Event {
        detail: Some(event::Detail::Note("mine".to_owned())),
        ..Default::default()
    };
    event.clear_output_only();
    assert!(matches!(event.detail, Some(event::Detail::Note(ref n)) if n == "mine"));
}

#[test]
fn descends_into_a_oneof_member_that_carries_a_message() {
    let mut event = Event {
        detail: Some(event::Detail::Handoff(Box::new(carrier("UPS")))),
        ..Default::default()
    };
    event.clear_output_only();

    let Some(event::Detail::Handoff(handoff)) = &event.detail else {
        panic!("the member is not output-only, so it stays set");
    };
    assert_eq!(handoff.name, "UPS");
    assert_eq!(handoff.scac, "");
}
