//! Ported from `go.einride.tech/aip/ordering`'s descriptor-based validation.
//!
//! aip-go validates `order_by` paths against `library.Book`; lacking those
//! protos, the cases use the vendored `einride.example` fixtures — `Shipper` for
//! flat paths (with the well-known `Timestamp` for a nested message), `Site` for
//! a nested user message (`lat_lng`), and `syntax.v1.Message` for descent past a
//! scalar. Path resolution itself is exercised exhaustively by `aip-fieldmask`;
//! here we confirm `OrderBy` threads its paths through it and maps the error.

use aip_ordering::{Error, OrderBy, OrderByField};
use prost_reflect::MessageDescriptor;

fn descriptor(full_name: &str) -> MessageDescriptor {
    test_fixtures::message_descriptor(full_name).expect("fixture message type is in the pool")
}

fn order_by(s: &str) -> OrderBy {
    s.parse().expect("order_by parses")
}

#[test]
fn empty_order_by_is_valid_against_any_message() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    OrderBy::default()
        .validate_for_message(&shipper)
        .expect("an empty order_by names no fields to reject");
}

#[test]
fn accepts_known_flat_paths() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    order_by("name, display_name")
        .validate_for_message(&shipper)
        .expect("name and display_name are Shipper fields");
}

#[test]
fn rejects_unknown_flat_path() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = order_by("name, foo")
        .validate_for_message(&shipper)
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "foo"),
        other => panic!("expected UnknownField for `foo`, got {other:?}"),
    }
}

#[test]
fn accepts_nested_message_path() {
    // `create_time` is a `google.protobuf.Timestamp`; `seconds` is one of its fields.
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    order_by("create_time.seconds")
        .validate_for_message(&shipper)
        .expect("a path into a nested message is valid");
}

#[test]
fn rejects_unknown_nested_path() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = order_by("create_time.foo")
        .validate_for_message(&shipper)
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "create_time.foo"),
        other => panic!("expected UnknownField for `create_time.foo`, got {other:?}"),
    }
}

#[test]
fn accepts_nested_user_message_subfield() {
    // `lat_lng` is a `google.type.LatLng`; `latitude` is one of its fields.
    let site = descriptor("einride.example.freight.v1.Site");
    order_by("lat_lng.latitude, lat_lng.longitude")
        .validate_for_message(&site)
        .expect("a path into a nested user message is valid");
}

#[test]
fn rejects_descent_past_a_scalar() {
    // `display_name` is a `string` — it has no subfields to descend into.
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = order_by("display_name.foo")
        .validate_for_message(&shipper)
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "display_name.foo"),
        other => panic!("expected UnknownField for `display_name.foo`, got {other:?}"),
    }
}

#[test]
fn direction_does_not_affect_validation() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let ob = OrderBy {
        fields: vec![
            OrderByField {
                path: "name".to_owned(),
                desc: true,
            },
            OrderByField {
                path: "display_name".to_owned(),
                desc: false,
            },
        ],
    };
    ob.validate_for_message(&shipper)
        .expect("asc/desc is irrelevant to whether the path resolves");
}

#[test]
fn reports_the_first_unknown_field_among_several() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = order_by("name, nope, display_name")
        .validate_for_message(&shipper)
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "nope"),
        other => panic!("expected UnknownField for `nope`, got {other:?}"),
    }
}
