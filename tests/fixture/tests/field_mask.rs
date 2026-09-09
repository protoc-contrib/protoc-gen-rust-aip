//! Exercises the AIP-134 `update_mask` check.

use aip_fixture::proto::example::v1::{
    Address, Carrier, Shipment, UpdateShipmentRequest, UpdateShipmentRequestOwnedView,
};
use buffa::MessageField;

fn request(paths: &[&str]) -> UpdateShipmentRequest {
    UpdateShipmentRequest {
        shipment: MessageField::some(Shipment::default()),
        update_mask: MessageField::some(buffa_types::google::protobuf::FieldMask {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

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
        assert!(!Shipment::is_mutable_path(path), "{path}");
    }
}

#[test]
fn a_nested_path_is_walked_rather_than_looked_up() {
    // `carrier` is writable and Carrier::name is writable, so the path through
    // both is. Carrier::scac is OUTPUT_ONLY, so the path through it is not.
    assert!(Shipment::is_mutable_path("carrier"));
    assert!(Shipment::is_mutable_path("carrier.name"));
    assert!(!Shipment::is_mutable_path("carrier.scac"));
    assert_eq!(Carrier::MUTABLE_PATHS, ["name"]);
}

#[test]
fn a_checker_is_emitted_for_a_message_only_a_path_can_reach() {
    // Address declares no resource and no update request names it; it gets a
    // checker because Shipment::origin is a writable way down to it.
    assert_eq!(Address::MUTABLE_PATHS, ["line1", "city"]);
    assert!(Shipment::is_mutable_path("origin.city"));
}

#[test]
fn a_path_that_runs_past_a_leaf_is_rejected() {
    assert!(Shipment::is_mutable_path("reference"));
    assert!(!Shipment::is_mutable_path("reference.nonsense"));
}

#[test]
fn a_repeated_or_map_field_is_writable_but_not_a_way_down() {
    // Replacing the whole collection is writing it. Addressing one entry is
    // not something an AIP-134 mask does, so nothing below it resolves.
    assert!(Shipment::is_mutable_path("parcels"));
    assert!(!Shipment::is_mutable_path("parcels.label"));
    assert!(Shipment::is_mutable_path("parcels_by_code"));
    assert!(!Shipment::is_mutable_path("parcels_by_code.label"));
}

#[test]
fn an_unknown_path_is_rejected_rather_than_ignored() {
    assert!(!Shipment::is_mutable_path("shoe_size"));
    assert!(!Shipment::is_mutable_path(""));
}

#[test]
fn a_mask_of_writable_paths_passes() {
    assert!(
        request(&["reference", "carrier.name"])
            .validate_update_mask()
            .is_ok()
    );
}

#[test]
fn an_empty_mask_names_no_bad_path() {
    assert!(request(&[]).validate_update_mask().is_ok());
}

#[test]
fn an_unset_mask_names_no_bad_path() {
    let request = UpdateShipmentRequest {
        shipment: MessageField::some(Shipment::default()),
        ..Default::default()
    };
    assert!(request.validate_update_mask().is_ok());
}

#[test]
fn every_offending_path_is_reported_and_named() {
    let error = request(&["reference", "create_time", "carrier.scac"])
        .validate_update_mask()
        .unwrap_err();

    assert_eq!(error.violations.len(), 2);
    assert!(error.to_string().contains("`create_time`"));
    assert!(error.to_string().contains("`carrier.scac`"));
    // A defect in the validator, not in the request -- neither slot is set.
    assert!(error.compile_error.is_none());
    assert!(error.runtime_error.is_none());
}

#[test]
fn a_violation_points_at_the_path_that_was_rejected() {
    // Indistinguishable from a violation the protovalidate plugin emits: same
    // field-path spelling, so a client cannot tell which rules were transpiled
    // from buf.validate and which were generated from field_behavior.
    let error = request(&["reference", "create_time"])
        .validate_update_mask()
        .unwrap_err();

    assert_eq!(
        error.violations[0].field.to_string(),
        "update_mask.paths[1]"
    );
    assert_eq!(error.violations[0].rule_id, "update_mask.mutable_paths");
}

#[test]
fn the_check_is_emitted_for_the_view_a_handler_actually_holds() {
    // A connectrpc handler is passed a ServiceRequest that derefs to the view,
    // so an accessor only on the owned message is one it cannot reach. This is
    // what `views=true` is for.
    let owned = request(&["reference", "create_time"]);
    let borrowed = UpdateShipmentRequestOwnedView::from_owned(&owned).unwrap();
    let error = borrowed.view().validate_update_mask().unwrap_err();
    assert_eq!(error.violations.len(), 1);
    assert!(error.to_string().contains("`create_time`"));
}
