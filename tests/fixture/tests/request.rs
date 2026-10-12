//! Exercises the request-level `FieldBehavior` items: `missing_required_request_paths`
//! and `imply_update_mask`, generated on a service's create, update and batch
//! requests. They answer as aip-go's `MissingRequiredRequestPaths` and
//! `ImplyUpdateMask` do.

use aip::FieldBehavior;
use aip_fixture::proto::example::v1::{
    BatchCreateOrdersRequest, BatchOrdersResponse, BatchUpdateOrdersRequest, CreateOrderRequest,
    Destination, Order, OrderEnvelope, UpdateOrderRequest,
};
use buffa::MessageField;
use buffa_types::google::protobuf::FieldMask;

/// An order that sets every REQUIRED field.
fn complete() -> Order {
    Order {
        title: "t".to_owned(),
        confirmed: true,
        quantity: Some(1),
        destination: Destination {
            city: "Sofia".to_owned(),
            ..Default::default()
        }
        .into(),
        stops: vec![Destination::default()],
        ..Default::default()
    }
}

fn mask(paths: &[&str]) -> FieldMask {
    FieldMask {
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        ..Default::default()
    }
}

fn strings(paths: &[aip::FieldPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

/// An update of `order`, with `paths` as its mask, or none when `None`.
fn update(order: Order, paths: Option<&[&str]>) -> UpdateOrderRequest {
    UpdateOrderRequest {
        order: order.into(),
        update_mask: paths.map_or_else(MessageField::none, |paths| mask(paths).into()),
        ..Default::default()
    }
}

// --- missing_required_request_paths ------------------------------------------

#[test]
fn a_create_checks_every_required_field_of_its_resource() {
    let request = CreateOrderRequest {
        order: Order::default().into(),
        ..Default::default()
    };
    assert_eq!(
        strings(&request.missing_required_request_paths()),
        [
            "order.title",
            "order.confirmed",
            "order.quantity",
            "order.destination",
            "order.stops"
        ]
    );
}

#[test]
fn a_create_without_its_resource_reports_nothing() {
    // The request's own `required` on the resource field reports that.
    assert!(
        CreateOrderRequest::default()
            .missing_required_request_paths()
            .is_empty()
    );
}

#[test]
fn an_update_checks_only_what_its_mask_names() {
    let request = update(
        Order {
            title: String::new(),
            ..complete()
        },
        Some(&["quantity"]),
    );
    assert!(request.missing_required_request_paths().is_empty());

    let request = update(Order::default(), Some(&["title", "destination.city"]));
    assert_eq!(
        strings(&request.missing_required_request_paths()),
        ["order.title", "order.destination.city"]
    );
}

#[test]
fn an_unset_mask_names_nothing_rather_than_reading_as_a_create() {
    let request = update(Order::default(), None);
    assert!(request.missing_required_request_paths().is_empty());
}

#[test]
fn a_batch_reports_each_entry_under_its_index() {
    let request = BatchCreateOrdersRequest {
        requests: vec![
            CreateOrderRequest {
                order: complete().into(),
                ..Default::default()
            },
            CreateOrderRequest {
                order: Order {
                    confirmed: false,
                    ..complete()
                }
                .into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        strings(&request.missing_required_request_paths()),
        ["requests[1].order.confirmed"]
    );
}

#[test]
fn a_batch_update_checks_each_entry_against_its_own_mask() {
    let request = BatchUpdateOrdersRequest {
        requests: vec![
            update(Order::default(), Some(&["quantity"])),
            update(Order::default(), Some(&["title"])),
        ],
        ..Default::default()
    };
    assert_eq!(
        strings(&request.missing_required_request_paths()),
        ["requests[0].order.quantity", "requests[1].order.title"]
    );
}

#[test]
fn a_message_that_is_no_request_keeps_the_defaults() {
    // Holds a resource, but no method takes it.
    let envelope = OrderEnvelope {
        order: Order::default().into(),
        ..Default::default()
    };
    assert!(envelope.missing_required_request_paths().is_empty());
    let response = BatchOrdersResponse {
        orders: vec![Order::default()],
        ..Default::default()
    };
    assert!(response.missing_required_request_paths().is_empty());
}

// --- imply_update_mask --------------------------------------------------------

#[test]
fn implies_an_omitted_mask_from_the_resource() {
    let mut request = update(
        Order {
            title: "t".to_owned(),
            quantity: Some(0),
            ..Default::default()
        },
        None,
    );
    request.imply_update_mask();
    assert_eq!(
        request
            .update_mask
            .as_option()
            .map(|mask| mask.paths.clone()),
        Some(vec!["title".to_owned(), "quantity".to_owned()])
    );
}

#[test]
fn implies_an_empty_mask_too() {
    let mut request = update(
        Order {
            title: "t".to_owned(),
            ..Default::default()
        },
        Some(&[]),
    );
    request.imply_update_mask();
    assert_eq!(
        request
            .update_mask
            .as_option()
            .map(|mask| mask.paths.clone()),
        Some(vec!["title".to_owned()])
    );
}

#[test]
fn keeps_a_mask_the_client_sent() {
    let mut request = update(complete(), Some(&["confirmed"]));
    request.imply_update_mask();
    assert_eq!(
        request
            .update_mask
            .as_option()
            .map(|mask| mask.paths.clone()),
        Some(vec!["confirmed".to_owned()])
    );
}

#[test]
fn leaves_the_mask_alone_without_a_resource() {
    let mut request = UpdateOrderRequest::default();
    request.imply_update_mask();
    assert!(request.update_mask.as_option().is_none());
}

#[test]
fn implies_each_batch_entry_on_its_own() {
    let mut request = BatchUpdateOrdersRequest {
        requests: vec![
            update(
                Order {
                    title: "t".to_owned(),
                    ..Default::default()
                },
                None,
            ),
            update(complete(), Some(&["confirmed"])),
        ],
        ..Default::default()
    };
    request.imply_update_mask();
    let masks: Vec<Vec<String>> = request
        .requests
        .iter()
        .map(|entry| {
            entry
                .update_mask
                .as_option()
                .map(|mask| mask.paths.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        masks,
        [vec!["title".to_owned()], vec!["confirmed".to_owned()]]
    );
}

#[test]
fn imply_then_check_reports_what_the_implied_mask_writes() {
    // The implied mask names only what the client set, so an update that
    // omitted its mask can't clear a REQUIRED field by leaving it unset.
    let mut request = update(
        Order {
            quantity: Some(2),
            ..Default::default()
        },
        None,
    );
    request.imply_update_mask();
    assert!(request.missing_required_request_paths().is_empty());
}

#[test]
fn a_create_has_no_mask_to_imply() {
    let mut request = CreateOrderRequest {
        order: complete().into(),
        ..Default::default()
    };
    request.imply_update_mask();
    assert!(request.missing_required_request_paths().is_empty());
}
