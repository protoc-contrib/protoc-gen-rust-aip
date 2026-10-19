//! Exercises the generated `aip::FieldResolver`: a field's name, or names joined by `.`
//! through singular message fields, resolved to its `aip::FieldPath` with the
//! number and type from the schema, and a name that is not a field refused.

use aip::{FieldPath, FieldType, Step};
use aip_fixture::proto::example::v1::{Address, Carrier, Event, Parcel, Shipment};

fn steps(path: &FieldPath) -> Vec<Step> {
    path.steps().collect()
}

#[test]
fn a_field_resolves_to_its_path_with_its_number_and_type() {
    assert_eq!(
        steps(&FieldPath::of::<Shipment>("reference")),
        [Step::Field {
            name: "reference",
            number: 1,
            ty: FieldType::String
        }]
    );
    assert_eq!(
        steps(&FieldPath::of::<Shipment>("revision")),
        [Step::Field {
            name: "revision",
            number: 5,
            ty: FieldType::Int32
        }]
    );
}

#[test]
fn a_name_walks_through_a_singular_message_field() {
    let path = FieldPath::of::<Shipment>("carrier.scac");
    assert_eq!(path.to_string(), "carrier.scac");
    assert_eq!(
        steps(&path),
        [
            Step::Field {
                name: "carrier",
                number: 6,
                ty: FieldType::Message
            },
            Step::Field {
                name: "scac",
                number: 2,
                ty: FieldType::String
            }
        ]
    );
}

#[test]
fn a_message_field_alone_is_a_path() {
    assert_eq!(FieldPath::of::<Shipment>("origin").to_string(), "origin");
    assert_eq!(FieldPath::of::<Shipment>("parcels").to_string(), "parcels");
    assert_eq!(
        FieldPath::of::<Shipment>("parcels_by_code").to_string(),
        "parcels_by_code"
    );
}

#[test]
fn a_nested_message_resolves_its_own_fields() {
    assert_eq!(
        FieldPath::of::<Parcel>("carrier.name").to_string(),
        "carrier.name"
    );
    assert_eq!(FieldPath::of::<Address>("city").to_string(), "city");
    assert_eq!(FieldPath::of::<Carrier>("name").to_string(), "name");
}

#[test]
fn a_oneof_member_is_a_field_of_the_message() {
    assert_eq!(FieldPath::of::<Event>("note").to_string(), "note");
}

#[test]
fn a_name_through_a_field_that_cannot_be_walked_is_not_a_path() {
    use aip::FieldResolver as _;
    // Neither a repeated field nor a map is walked into, a scalar holds no
    // message, and a well-known type is not one this plugin generated.
    assert_eq!(Shipment::field_path("parcels.label"), None);
    assert_eq!(Shipment::field_path("parcels_by_code.label"), None);
    assert_eq!(Shipment::field_path("reference.length"), None);
    assert_eq!(Shipment::field_path("create_time.seconds"), None);
}

#[test]
fn a_name_that_is_not_a_field_is_not_a_path() {
    use aip::FieldResolver as _;
    assert_eq!(Shipment::field_path("nope"), None);
    assert_eq!(Shipment::field_path("carrier.nope"), None);
    assert_eq!(Shipment::field_path(""), None);
}

#[test]
#[should_panic(expected = "`nope` is not a field of")]
fn of_refuses_a_name_that_is_not_a_field() {
    let _ = FieldPath::of::<Shipment>("nope");
}
