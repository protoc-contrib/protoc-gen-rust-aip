//! Exercises the `aip::ResourceWrites` impls and the `PROCEDURES` table
//! emitted for aip-rs's `FieldBehaviorInterceptor`, through
//! `aip::request::behavior::validate` -- what the interceptor does to each request message --
//! and through the table's own payload steps.

use aip::request::behavior::validate;
use aip_fixture::aip_gen::example::v1::PROCEDURES;
use aip_fixture::proto::example::v1::{
    BatchCreateOrdersRequest, CreateLineRequest, CreateOrderRequest, Destination, Line, Order,
    UpdateOrderRequest,
};
use buffa::MessageField;
use buffa_types::{Empty, FieldMask};
use connectrpc::{CodecFormat, ConnectError, ErrorCode, Payload};

/// An order with every `REQUIRED` field set.
fn order() -> Order {
    Order {
        title: "Lamp".to_owned(),
        confirmed: true,
        quantity: Some(1),
        destination: MessageField::some(Destination {
            city: "Sofia".to_owned(),
            ..Default::default()
        }),
        stops: vec![Destination {
            city: "Plovdiv".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Each violation in `error`'s `buf.validate.Violations` detail, as its field
/// names joined by dots, a `[]` after a field with an index, and its rule id.
fn violations(error: &ConnectError) -> Vec<String> {
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    error
        .details
        .iter()
        .filter_map(|detail| {
            protovalidate_buffa::decode_violations(detail).expect("a valid detail")
        })
        .flat_map(|violations| violations.violations)
        .map(|violation| {
            let path: Vec<String> = violation
                .field
                .elements
                .iter()
                .map(|element| {
                    let name = element.field_name.clone().unwrap_or_default();
                    if element.subscript.is_some() {
                        format!("{name}[]")
                    } else {
                        name
                    }
                })
                .collect();
            format!(
                "{} {}",
                path.join("."),
                violation.rule_id.unwrap_or_default()
            )
        })
        .collect()
}

/// `request` as `validate` -- what the interceptor applies to each message --
/// leaves it for the handler, or the error it answers with.
fn prepared<M>(request: M) -> Result<M, ConnectError>
where
    M: buffa::Message
        + connectrpc::JsonSerialize
        + connectrpc::JsonDeserialize
        + Clone
        + aip::FieldBehavior
        + aip::ResourceWrites
        + protovalidate_buffa::Validate
        + 'static,
{
    let mut payload = Payload::from_message(request, CodecFormat::Proto);
    validate::<M>(&mut payload)?;
    Ok(payload.message::<M>()?.clone())
}

#[test]
fn procedures_name_every_method_of_the_package() {
    let paths: Vec<&str> = PROCEDURES.iter().map(|procedure| procedure.path).collect();
    assert_eq!(
        paths,
        [
            "/example.v1.Orders/CreateOrder",
            "/example.v1.Orders/CreateLine",
            "/example.v1.Orders/UpdateOrder",
            "/example.v1.Orders/BatchCreateOrders",
            "/example.v1.Orders/BatchUpdateOrders",
            "/example.v1.Orders/WatchOrders",
            "/example.v1.Orders/DeleteOrder",
            "/example.v1.Orders/PurgeOrders",
            "/example.v1.Shelf/ListVolumes",
            "/example.v1.Shelf/ListCovers",
            "/example.v1.Shelf/Browse",
            "/example.v1.Gadgets/ListGadgets",
        ]
    );
}

#[test]
fn a_create_without_an_id_gets_a_minted_uuidv7_and_no_name() {
    let request = CreateOrderRequest {
        order: MessageField::some(order()),
        ..Default::default()
    };
    let request = prepared(request).expect("a complete order");
    let id = uuid::Uuid::parse_str(&request.order_id).expect("a minted UUID");
    assert_eq!(id.get_version_num(), 7);
    assert!(request.order.name.is_empty(), "the name is the handler's");
}

#[test]
fn a_create_keeps_the_id_the_client_sent() {
    let request = CreateOrderRequest {
        order_id: "sent".to_owned(),
        order: MessageField::some(order()),
        ..Default::default()
    };
    let request = prepared(request).expect("a complete order");
    assert_eq!(request.order_id, "sent");
}

#[test]
fn a_nested_create_gets_an_id_and_its_output_only_parent_cleared() {
    let request = CreateLineRequest {
        parent: "orders/o1".to_owned(),
        line: MessageField::some(Line {
            parent: "client-sent, OUTPUT_ONLY".to_owned(),
            sku: "lamp-1".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let request = prepared(request).expect("a complete line");
    assert_ne!(request.line_id, "");
    assert_eq!(request.parent, "orders/o1");
    assert!(
        request.line.parent.is_empty(),
        "OUTPUT_ONLY, and not copied on"
    );
    assert_eq!(request.line.name, "");
}

#[test]
fn a_create_reports_protovalidate_and_required_violations_together() {
    let request = CreateLineRequest {
        line: MessageField::some(Line::default()),
        ..Default::default()
    };
    let error = prepared(request).expect_err("no parent, no sku");
    assert_eq!(violations(&error), ["parent required", "line.sku required"]);
}

#[test]
fn an_update_implies_an_omitted_mask_from_what_it_sets() {
    let request = UpdateOrderRequest {
        order: MessageField::some(Order {
            title: "Lamp".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let request = prepared(request).expect("only the title is written, and it is set");
    assert_eq!(request.update_mask.paths, ["title"]);
}

#[test]
fn an_update_refuses_clearing_a_required_field_its_mask_names() {
    let request = UpdateOrderRequest {
        order: MessageField::some(Order::default()),
        update_mask: MessageField::some(FieldMask {
            paths: vec!["title".to_owned()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = prepared(request).expect_err("the mask names an empty title");
    assert_eq!(violations(&error), ["order.title required"]);
}

#[test]
fn a_batch_fills_in_each_entry_and_reports_each_under_its_index() {
    let batch = |title: &str| BatchCreateOrdersRequest {
        requests: vec![
            CreateOrderRequest {
                order: MessageField::some(order()),
                ..Default::default()
            },
            CreateOrderRequest {
                order: MessageField::some(Order {
                    title: title.to_owned(),
                    ..order()
                }),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let request = prepared(batch("Desk")).expect("every order has a title");
    assert!(
        request
            .requests
            .iter()
            .all(|entry| !entry.order_id.is_empty())
    );

    let error = prepared(batch("")).expect_err("the second order has no title");
    assert_eq!(violations(&error), ["requests[].order.title required"]);
}

#[test]
fn a_required_violation_carries_field_numbers_and_types() {
    let request = BatchCreateOrdersRequest {
        requests: vec![
            CreateOrderRequest {
                order: MessageField::some(order()),
                ..Default::default()
            },
            CreateOrderRequest {
                order: MessageField::some(Order {
                    title: String::new(),
                    ..order()
                }),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let error = prepared(request).expect_err("the second order has no title");
    let violation = error
        .details
        .iter()
        .find_map(|detail| protovalidate_buffa::decode_violations(detail).expect("a valid detail"))
        .and_then(|violations| violations.violations.into_iter().next())
        .expect("one violation");
    let elements: Vec<_> = violation
        .field
        .elements
        .iter()
        .map(|element| {
            (
                element.field_name.clone().unwrap_or_default(),
                element.field_number,
                element.field_type.map(|ty| format!("{ty:?}")),
            )
        })
        .collect();
    assert_eq!(
        elements,
        [
            (
                "requests".to_owned(),
                Some(1),
                Some("TYPE_MESSAGE".to_owned())
            ),
            ("order".to_owned(), Some(2), Some("TYPE_MESSAGE".to_owned())),
            ("title".to_owned(), Some(2), Some("TYPE_STRING".to_owned())),
        ]
    );
    assert_eq!(
        format!("{:?}", violation.field.elements[0].subscript),
        "Some(Index(1))"
    );
}

#[test]
fn a_unary_procedure_prepares_the_request_it_receives() {
    let procedure = PROCEDURES
        .iter()
        .find(|procedure| procedure.path == "/example.v1.Orders/CreateOrder")
        .expect("CreateOrder is listed");
    let mut payload = Payload::from_message(
        CreateOrderRequest {
            order: MessageField::some(order()),
            ..Default::default()
        },
        CodecFormat::Proto,
    );
    (procedure.inbound)(&mut payload).expect("a complete order");
    let request = payload
        .message::<CreateOrderRequest>()
        .expect("still a CreateOrderRequest");
    assert_ne!(request.order_id, "");
}

#[test]
fn a_streaming_procedure_clears_input_only_on_what_it_sends() {
    let procedure = PROCEDURES
        .iter()
        .find(|procedure| procedure.path == "/example.v1.Orders/WatchOrders")
        .expect("WatchOrders is listed");
    let mut payload = Payload::from_message(
        Order {
            coupon: "SECRET".to_owned(),
            ..order()
        },
        CodecFormat::Proto,
    );
    (procedure.outbound)(&mut payload).expect("an Order");
    let order = payload.message::<Order>().expect("still an Order");
    assert_eq!(order.coupon, "");
    assert_eq!(order.title, "Lamp");
}

#[test]
fn a_well_known_type_is_handled_like_any_other_message() {
    for path in [
        "/example.v1.Orders/DeleteOrder",
        "/example.v1.Orders/PurgeOrders",
    ] {
        let procedure = PROCEDURES
            .iter()
            .find(|procedure| procedure.path == path)
            .expect("listed");
        let mut payload = Payload::from_message(Empty::default(), CodecFormat::Proto);
        (procedure.outbound)(&mut payload).expect("an Empty");
        payload.message::<Empty>().expect("still an Empty");
    }
    let purge = PROCEDURES
        .iter()
        .find(|procedure| procedure.path == "/example.v1.Orders/PurgeOrders")
        .expect("listed");
    let mut payload = Payload::from_message(Empty::default(), CodecFormat::Proto);
    (purge.inbound)(&mut payload).expect("an Empty request has nothing to refuse");
}
