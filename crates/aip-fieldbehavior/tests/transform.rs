//!
//! Each case builds [`DynamicMessage`] fixtures of
//! `einride.example.syntax.v1.FieldBehaviorMessage` or
//! `einride.example.freight.v1.Shipper` from JSON, runs the dynamic-core
//! transform, and compares the result for equality — exercising the dynamic
//! core through `test-fixtures` as directed by ADR-0009.

use aip_fieldbehavior::{clear_fields_dynamic, copy_fields_dynamic, get, has, FieldBehavior};
use prost_reflect::DynamicMessage;

const FB_MSG: &str = "einride.example.syntax.v1.FieldBehaviorMessage";
const SHIPPER: &str = "einride.example.freight.v1.Shipper";

fn fb(json: &str) -> DynamicMessage {
    test_fixtures::from_json(FB_MSG, json).expect("valid FieldBehaviorMessage JSON")
}

fn shipper(json: &str) -> DynamicMessage {
    test_fixtures::from_json(SHIPPER, json).expect("valid Shipper JSON")
}

// ── get / has ─────────────────────────────────────────────────────────────────

#[test]
fn get_returns_empty_for_unannotated_field() {
    let desc = test_fixtures::message_descriptor(SHIPPER).expect("Shipper in pool");
    let name_field = desc.get_field_by_name("name").expect("Shipper.name");
    assert!(get(&name_field).is_empty());
}

#[test]
fn get_returns_output_only_for_create_time() {
    let desc = test_fixtures::message_descriptor(SHIPPER).expect("Shipper in pool");
    let field = desc.get_field_by_name("create_time").expect("create_time");
    let behaviors = get(&field);
    assert_eq!(behaviors, vec![FieldBehavior::OutputOnly]);
}

#[test]
fn get_returns_required_for_display_name() {
    let desc = test_fixtures::message_descriptor(SHIPPER).expect("Shipper in pool");
    let field = desc
        .get_field_by_name("display_name")
        .expect("display_name");
    let behaviors = get(&field);
    assert_eq!(behaviors, vec![FieldBehavior::Required]);
}

#[test]
fn has_returns_true_for_matching_behavior() {
    let desc = test_fixtures::message_descriptor(SHIPPER).expect("Shipper in pool");
    let field = desc.get_field_by_name("create_time").expect("create_time");
    assert!(has(&field, FieldBehavior::OutputOnly));
    assert!(!has(&field, FieldBehavior::Required));
}

// ── clear_fields_dynamic ──────────────────────────────────────────────────────

#[test]
fn clear_fields_removes_output_only_leaves_required() {
    // Port of: TestClearFields / "clear fields with set field_behavior"
    // create_time is OUTPUT_ONLY → cleared; display_name is REQUIRED → kept.
    let mut msg = shipper(
        r#"{"name":"shippers/1","createTime":"2024-01-01T00:00:00Z","displayName":"Acme"}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = shipper(r#"{"name":"shippers/1","displayName":"Acme"}"#);
    assert_eq!(
        msg, expected,
        "OUTPUT_ONLY timestamps cleared, REQUIRED display_name kept"
    );
}

#[test]
fn clear_fields_recurses_into_nested_message() {
    // Port of: "clear field with set field_behavior on nested message"
    let mut msg = fb(
        r#"{"field":"field","optionalField":"optional","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional","outputOnlyField":"output_only"}}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(
        r#"{"field":"field","optionalField":"optional","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional"}}"#,
    );
    assert_eq!(msg, expected);
}

#[test]
fn clear_fields_recurses_multiple_levels() {
    // Port of: "clear field with set field_behavior on multiple levels of nested messages"
    let mut msg = fb(
        r#"{"field":"field","optionalField":"optional","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional","outputOnlyField":"output_only","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional","outputOnlyField":"output_only"}}}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(
        r#"{"field":"field","optionalField":"optional","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional","messageWithoutFieldBehavior":{"field":"field","optionalField":"optional"}}}"#,
    );
    assert_eq!(msg, expected);
}

#[test]
fn clear_fields_recurses_into_repeated_message() {
    // Port of: "clear fields with set field_behavior on repeated message"
    let mut msg = fb(
        r#"{"field":"field","optionalField":"optional","repeatedMessage":[{"field":"field","optionalField":"optional","outputOnlyField":"output_only"}],"stringList":["string"]}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(
        r#"{"field":"field","optionalField":"optional","repeatedMessage":[{"field":"field","optionalField":"optional"}],"stringList":["string"]}"#,
    );
    assert_eq!(msg, expected);
}

#[test]
fn clear_fields_clears_repeated_field_with_behavior() {
    // Port of: "clear repeated field with set field_behavior"
    // repeatedOutputOnlyMessage has OUTPUT_ONLY → the whole repeated field is cleared.
    let mut msg = fb(
        r#"{"field":"field","optionalField":"optional","repeatedMessage":[{"field":"field","optionalField":"optional"}],"repeatedOutputOnlyMessage":[{"field":"field","optionalField":"optional","outputOnlyField":"output_only"}]}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(
        r#"{"field":"field","optionalField":"optional","repeatedMessage":[{"field":"field","optionalField":"optional"}]}"#,
    );
    assert_eq!(msg, expected);
}

#[test]
fn clear_fields_recurses_into_map_values() {
    // Port of: "clear fields with set field_behavior on message in map"
    let mut msg = fb(
        r#"{"optionalField":"optional","mapOptionalMessage":{"key_1":{"optionalField":"optional","outputOnlyField":"output_only"}},"stringMap":{"string_key":"string"}}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(
        r#"{"optionalField":"optional","mapOptionalMessage":{"key_1":{"optionalField":"optional"}},"stringMap":{"string_key":"string"}}"#,
    );
    assert_eq!(msg, expected);
}

#[test]
fn clear_fields_clears_map_field_with_behavior() {
    // Port of: "clear map field with set field_behavior"
    // mapOutputOnlyMessage has OUTPUT_ONLY → the whole map field is cleared.
    let mut msg = fb(
        r#"{"optionalField":"optional","mapOutputOnlyMessage":{"key_1":{"outputOnlyField":"output_only"}}}"#,
    );
    clear_fields_dynamic(&mut msg, &[FieldBehavior::OutputOnly]);

    let expected = fb(r#"{"optionalField":"optional"}"#);
    assert_eq!(msg, expected);
}

// ── copy_fields_dynamic ───────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "different types")]
fn copy_fields_panics_on_type_mismatch() {
    // Port of: TestCopyFields / "different types"
    let mut dst = shipper("{}");
    let src = test_fixtures::from_json("einride.example.freight.v1.Site", "{}")
        .expect("valid Site fixture");
    copy_fields_dynamic(&mut dst, &src, &[FieldBehavior::Required]);
}

#[test]
fn copy_fields_copies_output_only_from_src_to_dst() {
    // Copies OUTPUT_ONLY fields (server-set timestamps) from one Shipper to another.
    let mut dst = shipper(r#"{"name":"shippers/1","displayName":"Acme"}"#);
    let src = shipper(
        r#"{"name":"shippers/1","createTime":"2024-01-01T00:00:00Z","updateTime":"2024-06-01T00:00:00Z","displayName":"Acme"}"#,
    );
    copy_fields_dynamic(&mut dst, &src, &[FieldBehavior::OutputOnly]);

    let expected = shipper(
        r#"{"name":"shippers/1","createTime":"2024-01-01T00:00:00Z","updateTime":"2024-06-01T00:00:00Z","displayName":"Acme"}"#,
    );
    assert_eq!(dst, expected);
}

#[test]
fn copy_fields_clears_dst_when_src_field_absent() {
    // If OUTPUT_ONLY field is absent in src, it is cleared in dst.
    let mut dst = shipper(
        r#"{"name":"shippers/1","createTime":"2024-01-01T00:00:00Z","displayName":"Acme"}"#,
    );
    let src = shipper(r#"{"name":"shippers/1","displayName":"Acme"}"#);
    copy_fields_dynamic(&mut dst, &src, &[FieldBehavior::OutputOnly]);

    let expected = shipper(r#"{"name":"shippers/1","displayName":"Acme"}"#);
    assert_eq!(
        dst, expected,
        "create_time in dst should be cleared because src lacks it"
    );
}
