//! Validate `google.api.resource_reference` fields against the real freight
//! example protos, exercising the [Dynamic core][adr9]
//! `validate_resource_references_dynamic` through `test-fixtures` (JSON →
//! `DynamicMessage`). Ports aip-go's `TestValidateResourceReferences`.
//!
//! [adr9]: ../../../docs/adr/0009-reflective-typed-message-api.md

use aip_reflect::validate_resource_references_dynamic;

/// Builds a `DynamicMessage` of `full_name` from canonical (camelCase) JSON and
/// runs the dynamic-core validator over it.
fn validate(full_name: &str, json: &str) -> Result<(), aip_reflect::Error> {
    let message = test_fixtures::from_json(full_name, json).expect("valid JSON fixture");
    validate_resource_references_dynamic(&message)
}

const SHIPMENT: &str = "einride.example.freight.v1.Shipment";
const BATCH_GET_SITES: &str = "einride.example.freight.v1.BatchGetSitesRequest";
const CREATE_SHIPMENT: &str = "einride.example.freight.v1.CreateShipmentRequest";

#[test]
fn empty_message_passes() {
    validate(SHIPMENT, "{}").expect("an empty message has no references to check");
}

#[test]
fn valid_references_pass() {
    validate(
        SHIPMENT,
        r#"{"originSite": "shippers/1/sites/1", "destinationSite": "shippers/1/sites/2"}"#,
    )
    .expect("both sites match the Site pattern");
}

#[test]
fn empty_repeated_passes() {
    validate(BATCH_GET_SITES, "{}").expect("no names to check");
}

#[test]
fn valid_repeated_references_pass() {
    validate(
        BATCH_GET_SITES,
        r#"{"names": ["shippers/1/sites/1", "shippers/1/sites/2"]}"#,
    )
    .expect("every name matches the Site pattern");
}

#[test]
fn invalid_reference_is_rejected() {
    // `shippers/1` is a valid Shipper name but not a Site, which is the declared
    // reference type for `origin_site`.
    let err = validate(
        SHIPMENT,
        r#"{"originSite": "shippers/1", "destinationSite": "shippers/1/sites/2"}"#,
    )
    .expect_err("origin_site does not name a Site");
    assert_eq!(
        err.to_string(),
        "value `shippers/1` of field origin_site is not a valid resource reference \
         for freight-example.einride.tech/Site",
    );
}

#[test]
fn invalid_nested_reference_reports_dotted_path() {
    let err = validate(
        CREATE_SHIPMENT,
        r#"{"shipment": {"originSite": "shippers/1", "destinationSite": "shippers/1/sites/2"}}"#,
    )
    .expect_err("nested origin_site does not name a Site");
    assert_eq!(
        err.to_string(),
        "value `shippers/1` of field shipment.origin_site is not a valid resource reference \
         for freight-example.einride.tech/Site",
    );
}

#[test]
fn invalid_repeated_reference_reports_indexed_path() {
    let err = validate(
        BATCH_GET_SITES,
        r#"{"names": ["shippers/1/sites/1", "shippers/1"]}"#,
    )
    .expect_err("names[1] does not name a Site");
    assert_eq!(
        err.to_string(),
        "value `shippers/1` of field names[1] is not a valid resource reference \
         for freight-example.einride.tech/Site",
    );
}
