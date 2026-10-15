//! The test schema, compiled by `buffa` and extended by this plugin.
//!
//! Nothing here is hand-written apart from the two mount points below. The
//! crate exists so the plugin's output is type-checked against real
//! buffa-generated messages and exercised by [`tests/resource.rs`], which is
//! the only way to catch output that is well-formed Rust but wrong.

/// The buffa-generated message types.
pub mod proto {
    #![allow(
        clippy::all,
        clippy::pedantic,
        warnings,
        reason = "buffa-build generated code -- upstream codegen style, not ours to police"
    )]

    include!(concat!(env!("OUT_DIR"), "/proto/_include.rs"));
}

/// The AIP helpers this plugin emits for that schema.
pub mod aip_gen {
    include!(concat!(env!("OUT_DIR"), "/aip/mod.rs"));
}

/// protovalidate-buffa's `Validate`, which the `PROCEDURES` table needs on
/// every request but a well-known type, unary or streamed, and a consumer
/// generates with
/// protoc-gen-protovalidate-buffa. Written by hand here: every request passes,
/// except a `CreateLineRequest` without a `parent`, so the tests can see
/// protovalidate's violations merged with the `REQUIRED` ones the interceptor
/// adds.
mod validate {
    use protovalidate_buffa::{FieldPath, FieldPathElement, Validate, ValidationError, Violation};

    use crate::proto::example::v1::{
        BatchCreateOrdersRequest, BatchUpdateOrdersRequest, BrowseRequest, CreateLineRequest,
        CreateOrderRequest, DeleteOrderRequest, ListCoversRequest, ListGadgetsRequest,
        ListVolumesRequest, UpdateOrderRequest, WatchOrdersRequest,
    };

    macro_rules! valid {
        ($($request:ty),* $(,)?) => {
            $(impl Validate for $request {
                fn validate(&self) -> Result<(), ValidationError> {
                    Ok(())
                }
            })*
        };
    }

    valid!(
        BatchCreateOrdersRequest,
        BatchUpdateOrdersRequest,
        BrowseRequest,
        CreateOrderRequest,
        DeleteOrderRequest,
        ListCoversRequest,
        ListGadgetsRequest,
        ListVolumesRequest,
        UpdateOrderRequest,
        WatchOrdersRequest,
    );

    impl Validate for CreateLineRequest {
        fn validate(&self) -> Result<(), ValidationError> {
            if !self.parent.is_empty() {
                return Ok(());
            }
            Err(ValidationError {
                violations: vec![Violation {
                    field: FieldPath {
                        elements: vec![FieldPathElement {
                            field_name: Some("parent".into()),
                            ..Default::default()
                        }],
                    },
                    rule: FieldPath::default(),
                    rule_id: "required".into(),
                    message: "value is required".into(),
                    for_key: false,
                }],
                ..Default::default()
            })
        }
    }
}
