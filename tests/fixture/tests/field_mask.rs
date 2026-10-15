//! Exercises the generated `aip::Resource::implied_field_mask` — the field mask
//! AIP-134 implies when an update omits one — and `FieldBehavior::validate_fields`
//! with `REQUIRED`.
//!
//! Checking that a mask names only writable fields is *not* generated: that is
//! `(buf.validate.field).field_mask.in`, which protovalidate runs. See the
//! module docs on `emit::field_mask`.

use aip::{FieldBehavior, FieldBehaviorKind, FieldScope, Resource};
use aip_fixture::proto::example::v1::Shipment;

// --- implied_field_mask -----------------------------------------------------

use aip_fixture::proto::example::v1::{Address, Carrier, Meter, Parcel, Unit, meter};

#[test]
fn an_empty_resource_implies_an_empty_mask() {
    assert_eq!(
        Shipment::default().implied_field_mask().paths,
        Vec::<String>::new()
    );
    assert_eq!(
        Meter::default().implied_field_mask().paths,
        Vec::<String>::new()
    );
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
        shipment.implied_field_mask().paths,
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
    assert_eq!(shipment.implied_field_mask().paths, Vec::<String>::new());

    let meter = Meter {
        serial: "s1".to_owned(),
        ..Default::default()
    };
    assert_eq!(meter.implied_field_mask().paths, Vec::<String>::new());
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
    assert_eq!(meter.implied_field_mask().paths, ["threshold", "count"]);

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
        meter.implied_field_mask().paths,
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
    assert_eq!(meter.implied_field_mask().paths, ["level"]);
}

// --- validate_fields: REQUIRED ---------------------------------------------

use aip_fixture::proto::example::v1::{Destination, Geo, Node, Order, order};
use buffa::MessageField;

/// The paths a check reports, as strings: empty when it passes.
trait Reported {
    fn reported(self) -> Vec<String>;
}

impl Reported for Result<(), aip::FieldBehaviorError> {
    fn reported(self) -> Vec<String> {
        self.err().map_or_else(Vec::new, |error| {
            assert_eq!(error.kind, FieldBehaviorKind::Required);
            error.paths.iter().map(ToString::to_string).collect()
        })
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&value| value.to_owned()).collect()
}

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
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
    assert!(
        complete()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .is_ok()
    );
}

#[test]
fn a_reported_path_carries_each_field_number_and_type() {
    use aip::{FieldPath, FieldType};
    let order = Order {
        destination: destination("Sofia", Some(0.0)).into(),
        ..complete()
    };
    let error = order
        .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
        .unwrap_err();
    assert_eq!(
        error.paths,
        [FieldPath::field("destination", 5, FieldType::Message)
            .join(FieldPath::field("geo", 3, FieldType::Message))
            .join(FieldPath::field("latitude", 1, FieldType::Double))]
    );
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
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["confirmed"]
    );
}

#[test]
fn descends_into_a_set_message_at_any_depth() {
    let order = Order {
        destination: destination("", Some(0.0)).into(),
        ..complete()
    };
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["destination.city", "destination.geo.latitude"]
    );
}

#[test]
fn a_nested_message_is_checked_only_when_set() {
    // `billing` is not REQUIRED: unset, nothing beneath it is checked; set, its
    // own REQUIRED fields apply. Its `geo` is unset, so it is not entered.
    assert!(
        complete()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .is_ok()
    );
    let order = Order {
        billing: destination("", None).into(),
        ..complete()
    };
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["billing.city"]
    );
}

#[test]
fn a_repeated_field_is_present_when_not_empty_and_never_entered() {
    // `complete` holds one empty Destination in `stops`: its missing city is
    // not reported.
    assert!(
        complete()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .is_ok()
    );
}

#[test]
fn enters_the_oneof_member_that_is_set() {
    let order = Order {
        payment: Some(order::Payment::Voucher(Box::default())),
        ..complete()
    };
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["voucher.code"]
    );

    let order = Order {
        payment: Some(order::Payment::Card(String::new())),
        ..complete()
    };
    assert!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .is_ok()
    );
}

#[test]
fn follows_a_recursive_message_as_deep_as_it_is_set() {
    let order = Order {
        root: MessageField::some(Node {
            child: MessageField::some(Node::default()),
            ..Default::default()
        }),
        ..complete()
    };
    // Each message checks its own fields and passes the check on to what it
    // holds, so a set `child` is checked like `root`.
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["root.label", "root.child.label"]
    );
}

#[test]
fn an_update_checks_only_the_fields_its_mask_names() {
    // Nothing REQUIRED is set, but only `title` is named.
    let paths = mask(&["title"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["title"]
    );
    // A mask naming no REQUIRED field asks for none.
    let paths = mask(&["billing", "legs"]);
    assert!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .is_ok()
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
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
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
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["destination.city"]
    );
    let paths = mask(&["destination.geo"]);
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
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
        order
            .validate_fields(
                FieldBehaviorKind::Required,
                FieldScope::Mask(&update_mask.paths)
            )
            .reported(),
        ["confirmed"]
    );
}

#[test]
fn a_nested_path_is_checked_even_when_its_parent_is_unset() {
    // The update writes `destination.city` as empty, clearing a REQUIRED field.
    let paths = mask(&["destination.city"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["destination.city"]
    );
    // Two levels down, through two unset messages.
    let paths = mask(&["destination.geo.latitude"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["destination.geo.latitude"]
    );
    // Through a message that is not REQUIRED itself.
    let paths = mask(&["billing.city"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["billing.city"]
    );
    // Through a oneof member that is not the one set.
    let paths = mask(&["voucher.code"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["voucher.code"]
    );
    let order = Order {
        payment: Some(order::Payment::Card("4242".to_owned())),
        ..Default::default()
    };
    assert_eq!(
        order
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["voucher.code"]
    );
}

#[test]
fn a_nested_path_to_a_field_that_is_not_required_reports_nothing() {
    let paths = mask(&["billing.line2", "destination.line2"]);
    assert!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .is_ok()
    );
}

#[test]
fn a_create_does_not_enter_an_unset_message() {
    // Only the REQUIRED fields of the order itself; nothing beneath the unset
    // `destination`, `billing`, `voucher` or `root`.
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
}

#[test]
fn an_unset_message_named_whole_reports_only_itself() {
    // REQUIRED itself: reported at its level, nothing beneath it.
    let paths = mask(&["destination"]);
    assert_eq!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .reported(),
        ["destination"]
    );
    // Not REQUIRED: nothing at all.
    let paths = mask(&["billing"]);
    assert!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .is_ok()
    );
    // The same one level down: `destination.geo` is named whole and is not
    // REQUIRED, so its `latitude` is not checked.
    let paths = mask(&["destination.geo"]);
    assert!(
        Order::default()
            .validate_fields(FieldBehaviorKind::Required, FieldScope::Mask(&paths))
            .is_ok()
    );
}

// --- FieldBehavior as one generic layer -------------------------------------

/// What a server layer over every request does, without knowing the type.
fn admit<T: FieldBehavior>(message: &mut T) -> Vec<String> {
    message.clear_fields(FieldBehaviorKind::OutputOnly);
    message
        .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
        .reported()
}

#[test]
fn one_generic_layer_handles_every_generated_message() {
    // Carrier only has a field to clear: its check is the trait's default.
    let mut carrier = Carrier {
        name: "DHL".to_owned(),
        scac: "SERVER".to_owned(),
        ..Default::default()
    };
    assert_eq!(admit(&mut carrier), Vec::<String>::new());
    assert_eq!(carrier.scac, "");

    // A resource gets its own checks through the same layer.
    assert_eq!(
        admit(&mut Order::default()),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
}

#[test]
fn the_trait_is_dyn_compatible() {
    let mut order = Order::default();
    let behavior: &mut dyn FieldBehavior = &mut order;
    behavior.clear_fields(FieldBehaviorKind::OutputOnly);
    assert_eq!(
        behavior
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .reported(),
        ["title", "confirmed", "quantity", "destination", "stops"]
    );
}

// --- every message, and Resource --------------------------------------------

#[test]
fn a_message_with_no_behavior_still_implements_field_behavior() {
    // `Address` annotates nothing: its impl is empty, the trait's defaults.
    fn behaves<T: FieldBehavior>(message: &mut T) -> bool {
        message.clear_fields(FieldBehaviorKind::OutputOnly);
        message
            .validate_fields(FieldBehaviorKind::Required, FieldScope::All)
            .is_ok()
    }
    let mut address = Address {
        line1: "1 Main St".to_owned(),
        ..Default::default()
    };
    assert!(behaves(&mut address));
    assert_eq!(address.line1, "1 Main St");
}

#[test]
fn parse_name_reads_the_identifier_into_the_typed_name() {
    use aip::ResourceName;

    let shipment = Shipment {
        name: "shipments/s1".to_owned(),
        ..Default::default()
    };
    let name = shipment.parse_name().expect("a shipment name");
    assert_eq!(name.to_string(), "shipments/s1");
    assert_eq!(name.pattern(), "shipments/{shipment}");

    let shipment = Shipment {
        name: "parcels/p1".to_owned(),
        ..Default::default()
    };
    let error = shipment.parse_name().unwrap_err();
    assert_eq!(error.path().to_string(), "name");
}
