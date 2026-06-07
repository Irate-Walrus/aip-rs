//! Type-checking the timestamp/duration overloads and reflective enum
//! identifiers — the slices `check.rs` deliberately defers (issue #14).
//!
//! Timestamp/duration comparison is pure (it turns on declared identifier types
//! only); the enum cases declare an `enum_ident` from an `EnumDescriptor`
//! fixture in the shared test harness (`einride.example.syntax.v1.Enum`, whose
//! values are `ENUM_UNSPECIFIED` / `ENUM_ONE` / `ENUM_TWO`).

use aip_filtering::Type::{Duration, Timestamp};
use aip_filtering::{check, Declarations, DeclarationsBuilder, Error};
use prost_reflect::EnumDescriptor;

/// Build declarations on top of the standard function set.
fn decls(build: impl FnOnce(DeclarationsBuilder) -> DeclarationsBuilder) -> Declarations {
    build(Declarations::builder().standard_functions())
        .build()
        .expect("declarations build")
}

/// The vendored example enum, declared as the filterable identifier `category`.
fn example_enum() -> EnumDescriptor {
    test_fixtures::enum_descriptor("einride.example.syntax.v1.Enum")
        .expect("the example enum is in the fixture pool")
}

fn enum_decls() -> Declarations {
    Declarations::builder()
        .standard_functions()
        .enum_ident("category", example_enum())
        .build()
        .expect("declarations build")
}

#[test]
fn accepts_timestamp_and_duration_overloads() {
    let cases: Vec<(&str, Declarations)> = vec![
        // A timestamp field compared to an RFC3339 string literal — the
        // timestamp/string overload on each comparison operator.
        (
            r#"create_time > "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time >= "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time < "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time <= "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time = "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time != "2024-01-01T00:00:00Z""#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        // Two timestamp fields — the timestamp/timestamp overload.
        (
            "start_time < end_time",
            decls(|b| {
                b.ident("start_time", Timestamp)
                    .ident("end_time", Timestamp)
            }),
        ),
        // The `timestamp(...)` constructor lifts a string to a timestamp, which
        // then compares timestamp/timestamp.
        (
            r#"create_time = timestamp("2024-01-01T00:00:00Z")"#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        (
            r#"create_time > timestamp("2024-01-01T00:00:00Z")"#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        // A duration field has no string overload: it compares via `duration(...)`.
        (
            r#"ttl > duration("3600s")"#,
            decls(|b| b.ident("ttl", Duration)),
        ),
        (
            r#"timeout >= duration("1.5s")"#,
            decls(|b| b.ident("timeout", Duration)),
        ),
        // Two duration fields — the duration/duration overload.
        (
            "ttl = max_ttl",
            decls(|b| b.ident("ttl", Duration).ident("max_ttl", Duration)),
        ),
    ];
    for (filter, declarations) in cases {
        check(filter, &declarations)
            .unwrap_or_else(|e| panic!("filter {filter:?} should check: {e}"));
    }
}

#[test]
fn rejects_temporal_type_mismatches() {
    let cases: Vec<(&str, Declarations)> = vec![
        // A timestamp field has no integer overload.
        (
            "create_time > 123",
            decls(|b| b.ident("create_time", Timestamp)),
        ),
        // Unlike timestamps, durations have no string overload — a duration
        // string must go through the `duration(...)` constructor.
        (r#"ttl > "3600s""#, decls(|b| b.ident("ttl", Duration))),
        // Timestamps and durations are not cross-comparable.
        (
            r#"create_time = duration("3600s")"#,
            decls(|b| b.ident("create_time", Timestamp)),
        ),
    ];
    for (filter, declarations) in cases {
        match check(filter, &declarations) {
            Err(Error::Type(message)) => assert!(
                message.contains("no matching overload"),
                "filter {filter:?}: message {message:?} lacks 'no matching overload'"
            ),
            other => panic!("filter {filter:?}: expected a type error, got {other:?}"),
        }
    }
}

#[test]
fn accepts_enum_identifier_comparisons() {
    let declarations = enum_decls();
    for filter in [
        // The enum identifier compared to a bare enum value name.
        "category = ENUM_ONE",
        "category != ENUM_TWO",
        // The overload is symmetric in its operands.
        "ENUM_UNSPECIFIED = category",
        // Every value name of the enum resolves to the same enum type.
        "category = ENUM_ONE OR category = ENUM_TWO",
    ] {
        check(filter, &declarations)
            .unwrap_or_else(|e| panic!("filter {filter:?} should check: {e}"));
    }
}

#[test]
fn rejects_enum_value_as_string_literal() {
    // A quoted value is a string, not the bare enum value name, so it never
    // resolves to the enum type and has no matching `=` overload.
    let declarations = enum_decls();
    match check(r#"category = "ENUM_ONE""#, &declarations) {
        Err(Error::Type(message)) => assert!(
            message.contains("no matching overload"),
            "message {message:?} lacks 'no matching overload'"
        ),
        other => panic!("expected a type error, got {other:?}"),
    }
}

#[test]
fn rejects_undeclared_enum_value() {
    // Only the enum's own value names are declared; an unknown one is undeclared.
    let declarations = enum_decls();
    match check("category = ENUM_NOPE", &declarations) {
        Err(Error::UndeclaredIdent(name)) => assert_eq!(name, "ENUM_NOPE"),
        other => panic!("expected an undeclared-identifier error, got {other:?}"),
    }
}

#[test]
fn rejects_ordering_on_enum() {
    // `enum_ident` declares only `=` / `!=`, not the ordering operators.
    let declarations = enum_decls();
    match check("category > ENUM_ONE", &declarations) {
        Err(Error::Type(message)) => assert!(
            message.contains("no matching overload"),
            "message {message:?} lacks 'no matching overload'"
        ),
        other => panic!("expected a type error, got {other:?}"),
    }
}
