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

// --- changed_immutable_paths ------------------------------------------------------

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
    assert_eq!(update.changed_immutable_paths(&stored), ["serial"]);
}

#[test]
fn an_echoed_or_unset_immutable_field_is_not() {
    let stored = Meter {
        serial: "s1".to_owned(),
        ..Default::default()
    };
    // Echoed back unchanged.
    assert!(stored.clone().changed_immutable_paths(&stored).is_empty());
    // Left unset by the update.
    assert!(Meter::default().changed_immutable_paths(&stored).is_empty());
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
    assert!(update.changed_immutable_paths(&stored).is_empty());
}

// --- changed_immutable_paths: UNORDERED_LIST --------------------------------

use aip_fixture::proto::example::v1::{Destination, Geo, Node, Order, order};
use buffa::MessageField;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&value| value.to_owned()).collect()
}

fn order_with_lists(legs: &[&str], tags: &[&str]) -> Order {
    Order {
        legs: strings(legs),
        tags: strings(tags),
        ..Default::default()
    }
}

#[test]
fn an_unordered_list_in_another_order_is_unchanged() {
    let stored = order_with_lists(&[], &["a", "b", "c"]);
    let update = order_with_lists(&[], &["c", "a", "b"]);
    assert!(update.changed_immutable_paths(&stored).is_empty());
}

#[test]
fn an_unordered_list_compares_as_a_multiset() {
    let stored = order_with_lists(&[], &["a", "a", "b"]);
    // Same elements once each, different counts.
    let update = order_with_lists(&[], &["a", "b", "b"]);
    assert_eq!(update.changed_immutable_paths(&stored), ["tags"]);
    // A different length.
    let update = order_with_lists(&[], &["a", "b"]);
    assert_eq!(update.changed_immutable_paths(&stored), ["tags"]);
    // A different element.
    let update = order_with_lists(&[], &["a", "a", "c"]);
    assert_eq!(update.changed_immutable_paths(&stored), ["tags"]);
}

#[test]
fn an_ordered_list_in_another_order_is_changed() {
    let stored = order_with_lists(&["x", "y"], &[]);
    let update = order_with_lists(&["y", "x"], &[]);
    assert_eq!(update.changed_immutable_paths(&stored), ["legs"]);
}

// --- missing_required_paths -------------------------------------------------

fn destination(city: &str, latitude: Option<f64>) -> Destination {
    Destination {
        city: city.to_owned(),
        geo: match latitude {
            Some(latitude) => Geo {
                latitude,
                ..Default::default()
            }
            .into(),
            None => MessageField::none(),
        },
        ..Default::default()
    }
}

/// An order that sets every REQUIRED field, at every depth it is checked.
fn complete() -> Order {
    Order {
        title: "t".to_owned(),
        confirmed: true,
        quantity: Some(1),
        destination: destination("Sofia", Some(42.7)).into(),
        stops: vec![Destination::default()],
        ..Default::default()
    }
}

fn mask(paths: &[&str]) -> Vec<String> {
    strings(paths)
}

#[test]
fn a_create_must_set_every_required_field() {
    assert_eq!(
        Order::default().missing_required_paths(None),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
    assert!(complete().missing_required_paths(None).is_empty());
}

#[test]
fn reads_presence_as_protobuf_does_for_required() {
    let order = Order {
        // Explicit presence: zero is set.
        quantity: Some(0),
        // Implicit presence: a REQUIRED bool must be true.
        confirmed: false,
        ..complete()
    };
    assert_eq!(order.missing_required_paths(None), ["confirmed"]);
}

#[test]
fn descends_into_a_set_message_at_any_depth() {
    let order = Order {
        destination: destination("", Some(0.0)).into(),
        ..complete()
    };
    assert_eq!(
        order.missing_required_paths(None),
        ["destination.city", "destination.geo.latitude"]
    );
}

#[test]
fn a_nested_message_is_checked_only_when_set() {
    // `billing` is not REQUIRED: unset, nothing beneath it is checked; set, its
    // own REQUIRED fields apply. Its `geo` is unset, so it is not entered.
    assert!(complete().missing_required_paths(None).is_empty());
    let order = Order {
        billing: destination("", None).into(),
        ..complete()
    };
    assert_eq!(order.missing_required_paths(None), ["billing.city"]);
}

#[test]
fn a_repeated_field_is_present_when_not_empty_and_never_entered() {
    // `complete` holds one empty Destination in `stops`: its missing city is
    // not reported.
    assert!(complete().missing_required_paths(None).is_empty());
}

#[test]
fn enters_the_oneof_member_that_is_set() {
    let order = Order {
        payment: Some(order::Payment::Voucher(Box::default())),
        ..complete()
    };
    assert_eq!(order.missing_required_paths(None), ["voucher.code"]);

    let order = Order {
        payment: Some(order::Payment::Card(String::new())),
        ..complete()
    };
    assert!(order.missing_required_paths(None).is_empty());
}

#[test]
fn enters_a_recursive_message_once_per_path() {
    let order = Order {
        root: MessageField::some(Node {
            child: MessageField::some(Node::default()),
            ..Default::default()
        }),
        ..complete()
    };
    assert_eq!(order.missing_required_paths(None), ["root.label"]);
}

#[test]
fn an_update_checks_only_the_fields_its_mask_names() {
    // Nothing REQUIRED is set, but only `title` is named.
    let paths = mask(&["title"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["title"]
    );
    // A mask naming no REQUIRED field asks for none.
    let paths = mask(&["billing", "legs"]);
    assert!(
        Order::default()
            .missing_required_paths(Some(&paths))
            .is_empty()
    );
}

#[test]
fn a_mask_naming_a_message_checks_everything_beneath_it() {
    let order = Order {
        destination: destination("", Some(0.0)).into(),
        ..Default::default()
    };
    let paths = mask(&["destination"]);
    assert_eq!(
        order.missing_required_paths(Some(&paths)),
        ["destination.city", "destination.geo.latitude"]
    );
}

#[test]
fn a_nested_path_checks_just_that_field() {
    let order = Order {
        destination: destination("", Some(0.0)).into(),
        ..Default::default()
    };
    let paths = mask(&["destination.city"]);
    assert_eq!(
        order.missing_required_paths(Some(&paths)),
        ["destination.city"]
    );
    let paths = mask(&["destination.geo"]);
    assert_eq!(
        order.missing_required_paths(Some(&paths)),
        ["destination.geo.latitude"]
    );
}

#[test]
fn takes_a_field_mask_paths_as_they_are() {
    // The owned `FieldMask` holds `Vec<String>`: a request's mask is passed
    // with no conversion.
    let update_mask = buffa_types::google::protobuf::FieldMask {
        paths: mask(&["title", "confirmed"]),
        ..Default::default()
    };
    let order = Order {
        title: "t".to_owned(),
        ..Default::default()
    };
    assert_eq!(
        order.missing_required_paths(Some(&update_mask.paths)),
        ["confirmed"]
    );
}

#[test]
fn a_nested_path_is_checked_even_when_its_parent_is_unset() {
    // The update writes `destination.city` as empty, clearing a REQUIRED field.
    let paths = mask(&["destination.city"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["destination.city"]
    );
    // Two levels down, through two unset messages.
    let paths = mask(&["destination.geo.latitude"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["destination.geo.latitude"]
    );
    // Through a message that is not REQUIRED itself.
    let paths = mask(&["billing.city"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["billing.city"]
    );
    // Through a oneof member that is not the one set.
    let paths = mask(&["voucher.code"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["voucher.code"]
    );
    let order = Order {
        payment: Some(order::Payment::Card("4242".to_owned())),
        ..Default::default()
    };
    assert_eq!(order.missing_required_paths(Some(&paths)), ["voucher.code"]);
}

#[test]
fn a_nested_path_to_a_field_that_is_not_required_reports_nothing() {
    let paths = mask(&["billing.line2", "destination.line2"]);
    assert!(
        Order::default()
            .missing_required_paths(Some(&paths))
            .is_empty()
    );
}

#[test]
fn a_create_does_not_enter_an_unset_message() {
    // Only the REQUIRED fields of the order itself; nothing beneath the unset
    // `destination`, `billing`, `voucher` or `root`.
    assert_eq!(
        Order::default().missing_required_paths(None),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
}

#[test]
fn an_unset_message_named_whole_reports_only_itself() {
    // REQUIRED itself: reported at its level, nothing beneath it.
    let paths = mask(&["destination"]);
    assert_eq!(
        Order::default().missing_required_paths(Some(&paths)),
        ["destination"]
    );
    // Not REQUIRED: nothing at all.
    let paths = mask(&["billing"]);
    assert!(
        Order::default()
            .missing_required_paths(Some(&paths))
            .is_empty()
    );
    // The same one level down: `destination.geo` is named whole and is not
    // REQUIRED, so its `latitude` is not checked.
    let paths = mask(&["destination.geo"]);
    assert!(
        Order::default()
            .missing_required_paths(Some(&paths))
            .is_empty()
    );
}
