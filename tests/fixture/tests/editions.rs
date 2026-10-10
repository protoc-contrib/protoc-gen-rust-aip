//! Exercises an edition-2023 file, where buffa generates every singular scalar
//! as an `Option<T>`. That this file compiles is most of the test: each
//! accessor below reads an `Option<String>` a proto3 file has as a `String`.

use aip_fixture::proto::example::v1::{CreateGadgetRequest, Gadget, ListGadgetsRequest};

const ID: &str = "0190b8c6-6f6a-7d6e-9a3b-0c1d2e3f4a5b";

#[test]
fn reads_an_optional_resource_name() {
    let gadget = Gadget {
        name: Some(format!("gadgets/{ID}")),
        ..Default::default()
    };
    assert_eq!(gadget.parse_name().unwrap().gadget_id.to_string(), ID);
    // Unset reads as empty, which is not a name.
    assert!(Gadget::default().parse_name().is_err());
}

#[test]
fn reads_an_optional_reference() {
    let gadget = Gadget {
        owner: Some("publishers/p1".to_owned()),
        ..Default::default()
    };
    assert_eq!(gadget.parse_owner().unwrap().publisher_id, "p1");
}

#[test]
fn reads_an_optional_create_id() {
    assert_eq!(
        CreateGadgetRequest::default().parse_gadget_id().unwrap(),
        None
    );
    let request = CreateGadgetRequest {
        gadget_id: Some(ID.to_owned()),
        ..Default::default()
    };
    assert_eq!(
        request.parse_gadget_id().unwrap(),
        Some(uuid::Uuid::parse_str(ID).unwrap())
    );
}

#[test]
fn reads_an_optional_filter() {
    assert!(
        ListGadgetsRequest::default()
            .parse_filter()
            .unwrap()
            .is_none()
    );
    let request = ListGadgetsRequest {
        filter: Some(r#"label == "x""#.to_owned()),
        ..Default::default()
    };
    assert!(request.parse_filter().unwrap().is_some());
}

#[test]
fn implies_by_explicit_and_implicit_presence() {
    let gadget = Gadget {
        // Explicit: set counts, even to the zero value.
        label: Some(String::new()),
        count: Some(0),
        // Implicit: counts only when not the zero value.
        mode: String::new(),
        // Never writable, however set.
        name: Some("gadgets/x".to_owned()),
        serial: Some("s1".to_owned()),
        ..Default::default()
    };
    assert_eq!(gadget.implied_update_mask(), ["label", "count"]);

    let gadget = Gadget {
        mode: "fast".to_owned(),
        ..Default::default()
    };
    assert_eq!(gadget.implied_update_mask(), ["mode"]);
}

#[test]
fn clears_an_optional_output_only_field() {
    let mut gadget = Gadget {
        serial: Some("s1".to_owned()),
        ..Default::default()
    };
    gadget.clear_output_only();
    assert_eq!(gadget.serial, None);
}
