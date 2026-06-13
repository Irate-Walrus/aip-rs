//!
//! aip-go validates against `library.Book`/`CreateBookRequest`; lacking those
//! protos, the cases use the vendored `einride.example` fixtures — `Shipper`
//! for flat paths (with the well-known `Timestamp` for a nested message) and
//! `syntax.v1.Message` for repeated- and map-valued descents.

use aip_fieldmask::{is_full_replacement, validate, Error};
use prost_reflect::MessageDescriptor;
use prost_types::FieldMask;

fn descriptor(full_name: &str) -> MessageDescriptor {
    test_fixtures::message_descriptor(full_name).expect("fixture message type is in the pool")
}

fn mask(paths: &[&str]) -> FieldMask {
    FieldMask {
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

#[test]
fn accepts_empty_and_wildcard_masks() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    // An empty mask is valid (it carries no paths to reject).
    validate(&FieldMask::default(), &shipper).expect("empty mask is valid");
    // The lone full-replacement path is valid.
    validate(&mask(&["*"]), &shipper).expect("`*` alone is valid");
}

#[test]
fn rejects_wildcard_combined_with_other_paths() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = validate(&mask(&["*", "display_name"]), &shipper).unwrap_err();
    assert!(
        matches!(err, Error::WildcardNotAlone),
        "expected WildcardNotAlone, got {err:?}"
    );
}

#[test]
fn accepts_known_flat_paths() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    validate(&mask(&["name", "display_name"]), &shipper).expect("known fields are valid");
}

#[test]
fn rejects_unknown_flat_path() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = validate(&mask(&["name", "foo"]), &shipper).unwrap_err();
    match err {
        Error::UnknownPath { path, .. } => assert_eq!(path, "foo"),
        other => panic!("expected UnknownPath for `foo`, got {other:?}"),
    }
}

#[test]
fn accepts_nested_message_path() {
    // `create_time` is a `google.protobuf.Timestamp`; `seconds` is one of its fields.
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    validate(&mask(&["name", "create_time.seconds"]), &shipper)
        .expect("a path into a nested message is valid");
}

#[test]
fn rejects_unknown_nested_path() {
    let shipper = descriptor("einride.example.freight.v1.Shipper");
    let err = validate(&mask(&["name", "create_time.foo"]), &shipper).unwrap_err();
    match err {
        Error::UnknownPath { path, .. } => assert_eq!(path, "create_time.foo"),
        other => panic!("expected UnknownPath for `create_time.foo`, got {other:?}"),
    }
}

#[test]
fn descends_through_repeated_and_map_message_values() {
    let message = descriptor("einride.example.syntax.v1.Message");
    // A path may structurally descend into a repeated message's fields...
    validate(&mask(&["repeated_message.string"]), &message)
        .expect("repeated message field path is structurally valid");
    // ...and into a map's value-message fields.
    validate(&mask(&["map_string_message.string"]), &message)
        .expect("map value-message field path is valid");
}

#[test]
fn rejects_descent_past_a_scalar_or_scalar_map_value() {
    let message = descriptor("einride.example.syntax.v1.Message");
    // `string` is a scalar — it has no fields to descend into.
    assert!(validate(&mask(&["string.foo"]), &message).is_err());
    // A `map<string, string>` value is a scalar — it has no fields either.
    assert!(validate(&mask(&["map_string_string.foo"]), &message).is_err());
}

#[test]
fn is_full_replacement_detects_lone_wildcard() {
    assert!(is_full_replacement(&mask(&["*"])));
    assert!(!is_full_replacement(&FieldMask::default()));
    assert!(!is_full_replacement(&mask(&["*", "display_name"])));
    assert!(!is_full_replacement(&mask(&["display_name"])));
}
