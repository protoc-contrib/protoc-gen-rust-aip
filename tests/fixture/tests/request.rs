//! Exercises the request-level `FieldBehavior` items: `validate_required`,
//! `imply_resource_name`, `imply_update_mask` and the trait's `imply_request`,
//! generated on a service's create, update and batch requests. They answer as
//! aip-go's `ValidateRequired`, `ImplyResourceName`, `ImplyUpdateMask` and
//! `ImplyRequest` do.

use aip::FieldBehavior;
use aip_fixture::proto::example::v1::{
    BatchCreateOrdersRequest, BatchOrdersResponse, BatchUpdateOrdersRequest, CreateLineRequest,
    CreateOrderRequest, Destination, Geo, Line, Order, OrderEnvelope, UpdateOrderRequest, Voucher,
};
use buffa::MessageField;
use buffa_types::google::protobuf::FieldMask;

/// The paths a check reports, as strings: empty when it passes.
trait Reported {
    fn reported(self) -> Vec<String>;
}

impl Reported for Result<(), aip::RequiredError> {
    fn reported(self) -> Vec<String> {
        self.err().map_or_else(Vec::new, |error| {
            error.paths.iter().map(ToString::to_string).collect()
        })
    }
}

impl Reported for Result<(), aip::ImmutableError> {
    fn reported(self) -> Vec<String> {
        self.err().map_or_else(Vec::new, |error| {
            error.paths.iter().map(ToString::to_string).collect()
        })
    }
}

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

/// An update of `order`, with `paths` as its mask, or none when `None`.
fn update(order: Order, paths: Option<&[&str]>) -> UpdateOrderRequest {
    UpdateOrderRequest {
        order: order.into(),
        update_mask: paths.map_or_else(MessageField::none, |paths| mask(paths).into()),
        ..Default::default()
    }
}

// --- validate_required -------------------------------------------------------

#[test]
fn a_create_checks_every_required_field_of_its_resource() {
    let request = CreateOrderRequest {
        order: Order::default().into(),
        ..Default::default()
    };
    assert_eq!(
        request.validate_required().reported(),
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
    assert!(CreateOrderRequest::default().validate_required().is_ok());
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
    assert!(request.validate_required().is_ok());

    let request = update(Order::default(), Some(&["title", "destination.city"]));
    assert_eq!(
        request.validate_required().reported(),
        ["order.title", "order.destination.city"]
    );
}

#[test]
fn an_unset_mask_names_nothing_rather_than_reading_as_a_create() {
    let request = update(Order::default(), None);
    assert!(request.validate_required().is_ok());
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
        request.validate_required().reported(),
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
        request.validate_required().reported(),
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
    assert!(envelope.validate_required().is_ok());
    let response = BatchOrdersResponse {
        orders: vec![Order::default()],
        ..Default::default()
    };
    assert!(response.validate_required().is_ok());
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
    assert!(request.validate_required().is_ok());
}

#[test]
fn a_create_has_no_mask_to_imply() {
    let mut request = CreateOrderRequest {
        order: complete().into(),
        ..Default::default()
    };
    request.imply_update_mask();
    assert!(request.validate_required().is_ok());
}

// --- imply_resource_name -------------------------------------------------------

/// A `new_id` that hands out `id-1`, `id-2`, … and counts what it minted.
fn ids() -> (impl FnMut() -> String, std::rc::Rc<std::cell::Cell<usize>>) {
    let minted = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = std::rc::Rc::clone(&minted);
    let new_id = move || {
        counter.set(counter.get() + 1);
        format!("id-{}", counter.get())
    };
    (new_id, minted)
}

#[test]
fn a_create_mints_an_omitted_id_and_names_its_resource() {
    let (mut new_id, minted) = ids();
    let mut request = CreateOrderRequest {
        order: Order {
            name: "orders/client-sent".to_owned(),
            ..complete()
        }
        .into(),
        ..Default::default()
    };
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.order_id, "id-1");
    assert_eq!(
        request.order.name, "orders/id-1",
        "a client-sent name is replaced"
    );
    assert_eq!(minted.get(), 1);
}

#[test]
fn a_create_keeps_the_id_the_client_sent() {
    let (mut new_id, minted) = ids();
    let mut request = CreateOrderRequest {
        order_id: "mine".to_owned(),
        order: complete().into(),
        ..Default::default()
    };
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.order_id, "mine");
    assert_eq!(request.order.name, "orders/mine");
    assert_eq!(minted.get(), 0);
}

#[test]
fn a_create_without_its_resource_names_nothing() {
    let (mut new_id, minted) = ids();
    let mut request = CreateOrderRequest::default();
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.order_id, "");
    assert_eq!(minted.get(), 0);
}

#[test]
fn a_nested_resource_is_named_under_its_parent_and_takes_it() {
    let (mut new_id, _) = ids();
    let mut request = CreateLineRequest {
        parent: "orders/o1".to_owned(),
        line: Line {
            sku: "s".to_owned(),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    };
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.line_id, "id-1");
    assert_eq!(request.line.name, "orders/o1/lines/id-1");
    assert_eq!(request.line.parent, "orders/o1");
}

#[test]
fn a_batch_names_each_create_entry() {
    let (mut new_id, _) = ids();
    let mut request = BatchCreateOrdersRequest {
        requests: vec![
            CreateOrderRequest {
                order: complete().into(),
                ..Default::default()
            },
            CreateOrderRequest {
                order_id: "mine".to_owned(),
                order: complete().into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.requests[0].order.name, "orders/id-1");
    assert_eq!(request.requests[1].order.name, "orders/mine");
}

#[test]
fn an_update_keeps_the_name_it_was_sent() {
    let (mut new_id, minted) = ids();
    let mut request = update(
        Order {
            name: "orders/target".to_owned(),
            ..Default::default()
        },
        Some(&["title"]),
    );
    request.imply_resource_name(&mut new_id);
    assert_eq!(request.order.name, "orders/target");
    assert_eq!(minted.get(), 0);
}

// --- imply_request -------------------------------------------------------------

#[test]
fn imply_request_names_a_create_and_implies_an_update_mask() {
    let (mut new_id, _) = ids();
    let mut create = CreateOrderRequest {
        order: complete().into(),
        ..Default::default()
    };
    create.imply_request(&mut new_id);
    assert_eq!(create.order.name, "orders/id-1");
    assert!(create.validate_required().is_ok());

    let mut batch = BatchUpdateOrdersRequest {
        requests: vec![update(
            Order {
                title: "t".to_owned(),
                ..Default::default()
            },
            None,
        )],
        ..Default::default()
    };
    batch.imply_request(&mut new_id);
    assert_eq!(batch.requests[0].update_mask.paths, ["title"]);
    assert!(batch.validate_required().is_ok());
}

// --- every message implements the trait -----------------------------------------

/// Compiles only for a type that implements the trait.
fn implemented<T: FieldBehavior>() {}

#[test]
fn every_generated_message_implements_the_trait() {
    // No field behavior at all, or none any item needs: the empty impl.
    implemented::<Voucher>();
    implemented::<Geo>();
    implemented::<OrderEnvelope>();
    implemented::<BatchOrdersResponse>();

    // Their items are the defaults.
    let mut voucher = Voucher::default();
    assert!(voucher.validate_required().is_ok());
    voucher.imply_request(&mut || unreachable!("nothing to name"));
}
