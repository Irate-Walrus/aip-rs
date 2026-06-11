//! Evaluating a checked [`Filter`] against a message through the matcher's
//! **Dynamic core** ([`matches_dynamic`]) and **Typed facade** ([`matches`]).
//!
//! The crates' reflective test surface is JSON → a [`DynamicMessage`] via the
//! shared `test-fixtures` harness (ADR-0006). Two fixtures carry the full
//! operator set between them: `einride.example.syntax.v1.Message` (scalars, an
//! enum, a `map<string,string>`, a `repeated string`, a nested message) and the
//! freight `Site` (the `google.protobuf.Timestamp` fields and a nested
//! `LatLng`). The freight-server agreement test pins the same operators against
//! `aip-sql` + SQLite (issue #92).

use aip_filtering::Type::{Double, Int, List, Map, String, Timestamp};
use aip_filtering::{check, matches, matches_dynamic, Declarations};
use prost_reflect::{EnumDescriptor, ReflectMessage};

/// The vendored syntax enum (`ENUM_UNSPECIFIED` / `ENUM_ONE` / `ENUM_TWO`).
fn example_enum() -> EnumDescriptor {
    test_fixtures::enum_descriptor("einride.example.syntax.v1.Enum")
        .expect("the example enum is in the fixture pool")
}

/// Declarations over the rich `syntax.v1.Message` fixture: one filterable
/// identifier of each shape the matcher evaluates.
fn message_decls() -> Declarations {
    Declarations::builder()
        .standard_functions()
        .ident("string", String)
        .ident("int32", Int)
        .ident("double", Double)
        .ident("message.string", String)
        .ident("map_string_string", Map(Box::new(String), Box::new(String)))
        .ident("repeated_string", List(Box::new(String)))
        .enum_ident("enum", example_enum())
        .build()
}

/// Check `filter` against `declarations`, then evaluate it against the
/// `syntax.v1.Message` built from `json` via the **Dynamic core**.
fn run_message(json: &str, filter: &str) -> bool {
    let declarations = message_decls();
    let message = test_fixtures::from_json("einride.example.syntax.v1.Message", json)
        .expect("the fixture JSON parses");
    let checked = check(filter, &declarations).expect("the filter type-checks");
    matches_dynamic(&checked, &declarations, &message).expect("the matcher evaluates")
}

#[test]
fn matches_string_equality() {
    let json = r#"{"string": "hello"}"#;
    assert!(run_message(json, r#"string = "hello""#));
    assert!(!run_message(json, r#"string = "world""#));
    assert!(run_message(json, r#"string != "world""#));
}

#[test]
fn matches_numeric_comparisons() {
    let json = r#"{"int32": 5, "double": 1.5}"#;
    assert!(run_message(json, "int32 > 3"));
    assert!(!run_message(json, "int32 > 5"));
    assert!(run_message(json, "int32 >= 5"));
    assert!(run_message(json, "int32 = 5"));
    assert!(run_message(json, "double < 2.0"));
    assert!(!run_message(json, "double < 1.0"));
}

#[test]
fn mirrors_the_operator_when_the_field_is_on_the_right() {
    // `3 < int32` canonicalises to `int32 > 3`.
    let json = r#"{"int32": 5}"#;
    assert!(run_message(json, "3 < int32"));
    assert!(!run_message(json, "9 < int32"));
}

#[test]
fn matches_enum_by_value_name() {
    let json = r#"{"enum": "ENUM_ONE"}"#;
    assert!(run_message(json, "enum = ENUM_ONE"));
    assert!(!run_message(json, "enum = ENUM_TWO"));
    assert!(run_message(json, "enum != ENUM_TWO"));
    // An unset enum is its zero value `ENUM_UNSPECIFIED`.
    assert!(run_message(r#"{}"#, "enum = ENUM_UNSPECIFIED"));
}

#[test]
fn combines_with_and_or_not() {
    let json = r#"{"string": "hello", "int32": 5}"#;
    assert!(run_message(json, r#"string = "hello" AND int32 = 5"#));
    assert!(!run_message(json, r#"string = "hello" AND int32 = 9"#));
    assert!(run_message(json, r#"string = "nope" OR int32 = 5"#));
    assert!(!run_message(json, r#"string = "nope" OR int32 = 9"#));
    assert!(run_message(json, r#"NOT string = "world""#));
    assert!(!run_message(json, r#"NOT string = "hello""#));
}

#[test]
fn has_substring_is_ascii_case_insensitive() {
    // SQLite's default `LIKE` case-folds ASCII, so the matcher must too.
    let json = r#"{"string": "Hello"}"#;
    assert!(run_message(json, "string:ell"));
    assert!(run_message(json, "string:ELL"));
    assert!(run_message(json, "string:Hello"));
    assert!(!run_message(json, "string:xyz"));
}

#[test]
fn has_map_key_presence() {
    let json = r#"{"mapStringString": {"env": "prod", "team": "freight"}}"#;
    assert!(run_message(json, "map_string_string:env"));
    assert!(run_message(json, "map_string_string:team"));
    assert!(!run_message(json, "map_string_string:missing"));
}

#[test]
fn has_list_element_membership() {
    let json = r#"{"repeatedString": ["a", "b"]}"#;
    assert!(run_message(json, "repeated_string:a"));
    assert!(run_message(json, "repeated_string:b"));
    assert!(!run_message(json, "repeated_string:c"));
}

#[test]
fn map_member_access_reads_the_value_at_a_key() {
    let json = r#"{"mapStringString": {"env": "prod"}}"#;
    assert!(run_message(json, r#"map_string_string.env = "prod""#));
    assert!(!run_message(json, r#"map_string_string.env = "dev""#));
    // A missing key is absent → the comparison is unknown → the row is excluded.
    assert!(!run_message(json, r#"map_string_string.missing = "prod""#));
}

#[test]
fn resolves_a_nested_message_field() {
    let json = r#"{"message": {"string": "deep"}}"#;
    assert!(run_message(json, r#"message.string = "deep""#));
    assert!(!run_message(json, r#"message.string = "shallow""#));
}

#[test]
fn an_unset_nested_message_makes_the_comparison_unknown() {
    // `message` is unset, so `message.string` is absent → unknown, and a row
    // matches only on a definite `true` — under `NOT` the unknown stays unknown,
    // so the row is excluded either way (SQL three-valued logic).
    let json = r#"{}"#;
    assert!(!run_message(json, r#"message.string = "deep""#));
    assert!(!run_message(json, r#"NOT message.string = "deep""#));
}

// --- Timestamp / nested-numeric coverage over the freight `Site` fixture. ---

/// Declarations over the freight `Site` fixture: its timestamp fields and the
/// nested `LatLng` numeric path.
fn site_decls() -> Declarations {
    Declarations::builder()
        .standard_functions()
        .ident("display_name", String)
        .ident("create_time", Timestamp)
        .ident("delete_time", Timestamp)
        .ident("lat_lng.latitude", Double)
        .build()
}

/// Check `filter` against the site declarations, then evaluate it against the
/// `Site` built from `json`.
fn run_site(json: &str, filter: &str) -> bool {
    let declarations = site_decls();
    let site = test_fixtures::from_json("einride.example.freight.v1.Site", json)
        .expect("the fixture JSON parses");
    let checked = check(filter, &declarations).expect("the filter type-checks");
    matches_dynamic(&checked, &declarations, &site).expect("the matcher evaluates")
}

#[test]
fn compares_timestamps_lexicographically_as_rfc3339() {
    let json = r#"{"createTime": "2024-06-01T00:00:00Z"}"#;
    assert!(run_site(json, r#"create_time > "2024-01-01T00:00:00Z""#));
    assert!(!run_site(json, r#"create_time > "2999-01-01T00:00:00Z""#));
    assert!(run_site(json, r#"create_time = "2024-06-01T00:00:00Z""#));
    // The `timestamp(...)` constructor lifts the literal the same way.
    assert!(run_site(
        json,
        r#"create_time = timestamp("2024-06-01T00:00:00Z")"#
    ));
}

#[test]
fn has_timestamp_presence() {
    assert!(run_site(
        r#"{"createTime": "2024-06-01T00:00:00Z"}"#,
        "create_time:*"
    ));
    // An unset timestamp is not present.
    assert!(!run_site(
        r#"{"createTime": "2024-06-01T00:00:00Z"}"#,
        "delete_time:*"
    ));
}

#[test]
fn an_unset_timestamp_makes_a_comparison_unknown() {
    // `delete_time` is unset → the comparison is unknown → the row is excluded.
    let json = r#"{"createTime": "2024-06-01T00:00:00Z"}"#;
    assert!(!run_site(json, r#"delete_time > "2000-01-01T00:00:00Z""#));
}

#[test]
fn resolves_a_nested_numeric_path() {
    assert!(run_site(
        r#"{"latLng": {"latitude": 5.0}}"#,
        "lat_lng.latitude > 0"
    ));
    assert!(!run_site(
        r#"{"latLng": {"latitude": -5.0}}"#,
        "lat_lng.latitude > 0"
    ));
    // An unset `lat_lng` makes the nested numeric path absent → unknown.
    assert!(!run_site(r#"{}"#, "lat_lng.latitude > 0"));
}

#[test]
fn typed_facade_agrees_with_the_dynamic_core() {
    // `DynamicMessage` itself implements `ReflectMessage`, so the typed facade
    // `matches` (which transcodes `M → DynamicMessage`) runs over a fixture too —
    // exercising the facade path the headline interface uses.
    let declarations = message_decls();
    let message = test_fixtures::from_json("einride.example.syntax.v1.Message", r#"{"int32": 7}"#)
        .expect("the fixture JSON parses");
    let checked = check("int32 = 7", &declarations).expect("the filter type-checks");

    let via_facade = matches(&checked, &declarations, &message).expect("facade evaluates");
    let via_core = matches_dynamic(&checked, &declarations, &message).expect("core evaluates");
    assert!(via_facade);
    assert_eq!(via_facade, via_core);
    // Touch `ReflectMessage` so the import is load-bearing in the assertion above.
    let _ = message.descriptor();
}
