//! Table tests for the dynamic-core validation functions; each case uses
//! `test-fixtures` JSON → dynamic message fixtures.

use aip_fieldbehavior::{
    validate_immutable_not_changed_dynamic, validate_required_dynamic,
    validate_required_with_mask_dynamic, Error,
};
use prost_reflect::DynamicMessage;
use prost_types::FieldMask;

const SHIPPER: &str = "einride.example.freight.v1.Shipper";
const SHIPMENT: &str = "einride.example.freight.v1.Shipment";
const FB_MSG: &str = "einride.example.syntax.v1.FieldBehaviorMessage";

fn shipper(json: &str) -> DynamicMessage {
    test_fixtures::from_json(SHIPPER, json).expect("valid Shipper JSON")
}
fn shipment(json: &str) -> DynamicMessage {
    test_fixtures::from_json(SHIPMENT, json).expect("valid Shipment JSON")
}
fn fb(json: &str) -> DynamicMessage {
    test_fixtures::from_json(FB_MSG, json).expect("valid FieldBehaviorMessage JSON")
}
fn mask(paths: &[&str]) -> FieldMask {
    FieldMask {
        paths: paths.iter().map(|p| (*p).to_owned()).collect(),
    }
}

// ── validate_required_dynamic ─────────────────────────────────────────────────

#[test]
fn required_ok_when_present() {
    // Port of: TestValidateRequiredFields — GetShipmentRequest with a name is OK.
    // We use Shipper with a display_name (the one REQUIRED field on Shipper).
    let msg = shipper(r#"{"displayName":"Acme"}"#);
    assert!(validate_required_dynamic(&msg).is_ok());
}

#[test]
fn required_errors_on_missing_field() {
    // Port of: TestValidateRequiredFields — empty GetShipmentRequest.
    let msg = shipper("{}");
    let err = validate_required_dynamic(&msg).expect_err("display_name is required");
    match err {
        Error::RequiredFields { paths } => assert_eq!(paths, ["display_name"]),
        other => panic!("expected RequiredFields, got {other:?}"),
    }
}

#[test]
fn required_accumulates_every_missing_field() {
    // A bare Shipment is missing all six REQUIRED fields; the maskless validator
    // collects every path in declaration order rather than bailing on the first
    // (ADR-0007).
    let msg = shipment("{}");
    let err = validate_required_dynamic(&msg).expect_err("all REQUIRED fields are missing");
    match err {
        Error::RequiredFields { paths } => assert_eq!(
            paths,
            [
                "origin_site",
                "destination_site",
                "pickup_earliest_time",
                "pickup_latest_time",
                "delivery_earliest_time",
                "delivery_latest_time",
            ]
        ),
        other => panic!("expected RequiredFields, got {other:?}"),
    }
}

// ── validate_required_with_mask_dynamic ───────────────────────────────────────

#[test]
fn required_with_mask_ok_when_present() {
    let msg = shipper(r#"{"displayName":"Acme"}"#);
    assert!(validate_required_with_mask_dynamic(&msg, &mask(&["*"])).is_ok());
}

#[test]
fn required_with_mask_empty_mask_is_noop() {
    // Port of: "ok - empty mask" — missing required field but empty mask → OK.
    let msg = shipper("{}");
    assert!(validate_required_with_mask_dynamic(&msg, &mask(&[])).is_ok());
}

#[test]
fn required_with_mask_wildcard_catches_missing() {
    // Port of: "missing field" — wildcard mask, required field missing → error.
    let msg = shipper("{}");
    let err = validate_required_with_mask_dynamic(&msg, &mask(&["*"]))
        .expect_err("display_name is required");
    match err {
        Error::RequiredFields { paths } => assert_eq!(paths, ["display_name"]),
        other => panic!("expected RequiredFields, got {other:?}"),
    }
}

#[test]
fn required_with_mask_field_not_in_mask_is_ok() {
    // Port of: "missing but not in mask" — required field missing but not in mask → OK.
    let msg = shipper("{}");
    assert!(validate_required_with_mask_dynamic(&msg, &mask(&["name"])).is_ok());
}

#[test]
fn required_with_mask_catches_missing_nested_field() {
    // Port of: "missing nested" — an inner required field that IS in the mask.
    // Use Shipment which has REQUIRED origin_site and destination_site.
    let msg = shipment(r#"{"name":"shippers/s/shipments/m"}"#);
    let err = validate_required_with_mask_dynamic(&msg, &mask(&["origin_site"]))
        .expect_err("origin_site is required");
    match err {
        Error::RequiredFields { paths } => assert_eq!(paths, ["origin_site"]),
        other => panic!("expected RequiredFields, got {other:?}"),
    }
}

#[test]
fn required_with_mask_nested_not_in_mask_is_ok() {
    // Port of: "missing nested not in mask"
    // origin_site is REQUIRED and not set; mask covers only destination_site (also set)
    // → validation passes because origin_site is not in the mask.
    let msg = shipment(r#"{"name":"shippers/s/shipments/m","destinationSite":"sites/dest"}"#);
    assert!(
        validate_required_with_mask_dynamic(&msg, &mask(&["destination_site"])).is_ok(),
        "origin_site is required but not in mask, so no error expected"
    );
}

// ── validate_immutable_not_changed_dynamic ────────────────────────────────────

#[test]
fn immutable_ok_when_field_not_in_mask() {
    // Port of: "no error when immutable field not in mask"
    // The immutable_field in FieldBehaviorMessage is not in the mask.
    let old = fb(r#"{"immutableField":"value"}"#);
    let updated = fb(r#"{"immutableField":"changed"}"#);
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["field"])).is_ok());
}

#[test]
fn immutable_ok_when_field_unchanged() {
    // Port of: "no error when immutable field unchanged"
    let old = fb(r#"{"immutableField":"value"}"#);
    let updated = fb(r#"{"immutableField":"value","field":"other"}"#);
    assert!(validate_immutable_not_changed_dynamic(
        &old,
        &updated,
        &mask(&["immutable_field", "field"])
    )
    .is_ok());
}

#[test]
fn immutable_errors_when_field_changed() {
    // Port of: "error when immutable field changed"
    let old = fb(r#"{"immutableField":"original"}"#);
    let updated = fb(r#"{"immutableField":"changed"}"#);
    let err = validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["immutable_field"]))
        .expect_err("immutable_field change is rejected");
    match err {
        Error::ImmutableField { path } => assert_eq!(path, "immutable_field"),
        other => panic!("expected ImmutableField, got {other:?}"),
    }
}

#[test]
fn immutable_wildcard_catches_changed_field() {
    // Port of: "error when wildcard used and immutable field changed"
    let old = fb(r#"{"immutableField":"original"}"#);
    let updated = fb(r#"{"immutableField":"changed"}"#);
    let err = validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["*"]))
        .expect_err("wildcard mask + immutable change → error");
    assert!(matches!(err, Error::ImmutableField { .. }));
}

#[test]
fn immutable_wildcard_ok_when_field_unchanged() {
    // Port of: "no error when wildcard used but immutable field unchanged"
    let old = fb(r#"{"immutableField":"same","field":"x"}"#);
    let updated = fb(r#"{"immutableField":"same","field":"y"}"#);
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["*"])).is_ok());
}

#[test]
fn immutable_type_mismatch_returns_error() {
    // Port of: "error when different message types"
    let old = shipper("{}");
    let updated = test_fixtures::from_json("einride.example.freight.v1.Site", "{}")
        .expect("valid Site fixture");
    let err = validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["*"]))
        .expect_err("type mismatch is an error");
    assert!(matches!(err, Error::TypeMismatch { .. }));
}

#[test]
fn immutable_errors_on_changed_nested_in_repeated() {
    // Port of: "error when nested immutable field in repeated message changed"
    // Shipment.line_items[].external_reference_id is IMMUTABLE.
    let old = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    let updated = shipment(
        r#"{"lineItems":[{"title":"Item 1 Updated","externalReferenceId":"ref-1-changed"}]}"#,
    );
    let err = validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["line_items"]))
        .expect_err("immutable nested field change is rejected");
    assert!(matches!(err, Error::ImmutableField { .. }));
}

#[test]
fn immutable_ok_when_nested_in_repeated_unchanged() {
    // Port of: "no error when nested immutable field in repeated message unchanged"
    let old = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    let updated =
        shipment(r#"{"lineItems":[{"title":"Item 1 Updated","externalReferenceId":"ref-1"}]}"#);
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["line_items"])).is_ok());
}

#[test]
fn immutable_ok_when_new_repeated_element_added() {
    // Port of: "no error when new element added with immutable field set"
    let old = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    let updated = shipment(
        r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"},{"title":"Item 2","externalReferenceId":"ref-2"}]}"#,
    );
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["line_items"])).is_ok());
}

#[test]
fn immutable_ok_when_old_list_empty() {
    // Port of: "no error when old list is empty and updated has elements with immutable fields"
    let old = shipment(r#"{"lineItems":[]}"#);
    let updated = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["line_items"])).is_ok());
}

#[test]
fn immutable_ok_when_updated_list_empty() {
    // Port of: "no error when updated list is empty and old had elements"
    let old = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    let updated = shipment(r#"{"lineItems":[]}"#);
    assert!(validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["line_items"])).is_ok());
}

#[test]
fn immutable_errors_on_changed_nested_in_map() {
    // Port of: "error when nested immutable field in map message changed"
    let old = fb(r#"{"mapOptionalMessage":{"key1":{"field":"v1","immutableField":"imm-1"}}}"#);
    let updated = fb(
        r#"{"mapOptionalMessage":{"key1":{"field":"v1-updated","immutableField":"imm-1-changed"}}}"#,
    );
    let err =
        validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["map_optional_message"]))
            .expect_err("immutable nested map field change is rejected");
    assert!(matches!(err, Error::ImmutableField { .. }));
}

#[test]
fn immutable_ok_when_nested_in_map_unchanged() {
    // Port of: "no error when nested immutable field in map message unchanged"
    let old = fb(r#"{"mapOptionalMessage":{"key1":{"field":"v1","immutableField":"imm-1"}}}"#);
    let updated =
        fb(r#"{"mapOptionalMessage":{"key1":{"field":"v1-updated","immutableField":"imm-1"}}}"#);
    assert!(validate_immutable_not_changed_dynamic(
        &old,
        &updated,
        &mask(&["map_optional_message"]),
    )
    .is_ok());
}

#[test]
fn immutable_ok_when_old_singular_message_not_set() {
    // Port of: "no error when old singular message field not set and updated has immutable field"
    let old = fb(r#"{"field":"field"}"#);
    let updated =
        fb(r#"{"field":"field","messageWithoutFieldBehavior":{"immutableField":"value"}}"#);
    assert!(validate_immutable_not_changed_dynamic(
        &old,
        &updated,
        &mask(&["message_without_field_behavior"]),
    )
    .is_ok());
}

#[test]
fn immutable_ok_when_updated_singular_message_not_set() {
    // Port of: "no error when updated singular message field not set and old had immutable field"
    let old = fb(r#"{"field":"field","messageWithoutFieldBehavior":{"immutableField":"value"}}"#);
    let updated = fb(r#"{"field":"field"}"#);
    assert!(validate_immutable_not_changed_dynamic(
        &old,
        &updated,
        &mask(&["message_without_field_behavior"]),
    )
    .is_ok());
}

#[test]
fn immutable_errors_when_trying_to_unset_top_level() {
    // Port of: "error when trying to unset top-level immutable field"
    let old = fb(r#"{"immutableField":"ref-123"}"#);
    let updated = fb(r#"{}"#);
    let err = validate_immutable_not_changed_dynamic(&old, &updated, &mask(&["immutable_field"]))
        .expect_err("unsetting an immutable field is a change");
    assert!(matches!(err, Error::ImmutableField { .. }));
}

#[test]
fn immutable_prefix_mask_catches_nested_change() {
    // Port of: "error when nested field under immutable parent is in mask and changed"
    // Mask contains "line_items.external_reference_id" (nested path), not just "line_items".
    let old = shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1"}]}"#);
    let updated =
        shipment(r#"{"lineItems":[{"title":"Item 1","externalReferenceId":"ref-1-changed"}]}"#);
    let err = validate_immutable_not_changed_dynamic(
        &old,
        &updated,
        &mask(&["line_items.external_reference_id"]),
    )
    .expect_err("nested immutable field change via prefix mask is rejected");
    assert!(
        matches!(err, Error::ImmutableField { path } if path.contains("external_reference_id"))
    );
}
