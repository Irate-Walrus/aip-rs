//! Smoke test: the harness loads descriptors and round-trips a `DynamicMessage`
//! from JSON, with no `protoc` involved at any point.

use prost_reflect::ReflectMessage;
use test_fixtures::{from_json, message_descriptor, pool};

const SHIPPER: &str = "einride.example.freight.v1.Shipper";

#[test]
fn pool_exposes_vendored_example_messages() {
    let pool = pool();
    // One message from each vendored example package.
    assert!(pool.get_message_by_name(SHIPPER).is_some());
    assert!(pool
        .get_message_by_name("einride.example.syntax.v1.Message")
        .is_some());
    // A vendored googleapis import resolved.
    assert!(pool.get_message_by_name("google.type.LatLng").is_some());
}

#[test]
fn descriptor_lookup_reports_unknown_types() {
    assert!(message_descriptor(SHIPPER).is_some());
    assert!(message_descriptor("einride.example.freight.v1.DoesNotExist").is_none());

    let err = from_json("einride.example.freight.v1.DoesNotExist", "{}").unwrap_err();
    assert!(matches!(
        err,
        test_fixtures::FixtureError::UnknownMessage(_)
    ));
}

#[test]
fn shipper_round_trips_through_json() {
    // Exercises a scalar field and a well-known `Timestamp` (RFC 3339 string).
    let json = r#"{"name":"shippers/acme","displayName":"Acme Shipping","createTime":"2023-01-15T09:30:00Z"}"#;

    let message = from_json(SHIPPER, json).expect("build Shipper fixture from JSON");

    // The descriptor is the named message type.
    assert_eq!(message.descriptor().full_name(), SHIPPER);

    // A field reads back through reflection.
    let name = message.get_field_by_name("name").expect("name field");
    assert_eq!(name.as_str(), Some("shippers/acme"));

    // JSON -> DynamicMessage -> JSON is a faithful round-trip.
    let expected: serde_json::Value = serde_json::from_str(json).unwrap();
    let actual: serde_json::Value = serde_json::to_value(&message).unwrap();
    assert_eq!(actual, expected);
}
