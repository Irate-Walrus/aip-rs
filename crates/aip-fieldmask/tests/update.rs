//! Ported from `go.einride.tech/aip/fieldmask`'s `update_test.go`.
//!
//! Each case builds `src`/`dst`/`expected` as [`DynamicMessage`] fixtures of
//! `einride.example.syntax.v1.Message` from JSON, applies [`update`], and
//! compares the result for equality. The JSON encoding makes proto3 presence
//! explicit: a default-valued scalar (e.g. `bool: false`) is simply absent.
//!
//! 64-bit ints are JSON strings (`"int64": "222"`) and `bytes` are base64
//! (`[111]` is `"bw=="`, `[222]` is `"3g=="`), per the canonical protobuf JSON
//! mapping.

use aip_fieldmask::{update, Error};
use prost_reflect::DynamicMessage;
use prost_types::FieldMask;

const MESSAGE: &str = "einride.example.syntax.v1.Message";

/// Build a `syntax.v1.Message` fixture from JSON.
fn msg(json: &str) -> DynamicMessage {
    test_fixtures::from_json(MESSAGE, json).expect("valid syntax.v1.Message JSON fixture")
}

/// A single update table case: apply `paths` (the field mask) copying `src` into
/// `dst`, and assert the result equals `expected`.
struct Case {
    name: &'static str,
    paths: &'static [&'static str],
    src: &'static str,
    dst: &'static str,
    expected: &'static str,
}

fn run(cases: &[Case]) {
    for case in cases {
        let mask = FieldMask {
            paths: case.paths.iter().map(|p| (*p).to_owned()).collect(),
        };
        let src = msg(case.src);
        let mut dst = msg(case.dst);
        update(&mask, &mut dst, &src).expect("same-type update succeeds");
        assert_eq!(dst, msg(case.expected), "case: {}", case.name);
    }
}

#[test]
fn type_mismatch_returns_error_instead_of_panicking() {
    // aip-go panics on differing src/dst types; we return `Error::TypeMismatch`.
    let mut dst = test_fixtures::from_json("einride.example.freight.v1.Shipper", "{}").unwrap();
    let src = test_fixtures::from_json("einride.example.freight.v1.Site", "{}").unwrap();
    let err = update(&FieldMask::default(), &mut dst, &src).unwrap_err();
    assert!(
        matches!(err, Error::TypeMismatch { .. }),
        "expected TypeMismatch, got {err:?}"
    );
}

#[test]
fn full_replacement() {
    // `["*"]`: `dst` becomes an exact copy of `src` regardless of its prior state.
    run(&[
        Case {
            name: "scalars",
            paths: &["*"],
            src: r#"{"double":111,"float":111,"bool":true,"string":"111","bytes":"bw=="}"#,
            dst: r#"{"double":222,"float":222,"string":"222","bytes":"3g=="}"#,
            expected: r#"{"double":111,"float":111,"bool":true,"string":"111","bytes":"bw=="}"#,
        },
        Case {
            name: "repeated",
            paths: &["*"],
            src: r#"{"repeatedDouble":[111],"repeatedFloat":[111],"repeatedBool":[true],"repeatedString":["111"],"repeatedBytes":["bw=="]}"#,
            dst: r#"{"repeatedDouble":[222],"repeatedFloat":[222],"repeatedBool":[false],"repeatedString":["222"],"repeatedBytes":["3g=="]}"#,
            expected: r#"{"repeatedDouble":[111],"repeatedFloat":[111],"repeatedBool":[true],"repeatedString":["111"],"repeatedBytes":["bw=="]}"#,
        },
        Case {
            name: "nested",
            paths: &["*"],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{"message":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"message":{"string":"src"}}"#,
        },
        Case {
            name: "maps",
            paths: &["*"],
            src: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
            dst: r#"{"mapStringString":{"dst-key":"dst-value"},"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
            expected: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
        },
        Case {
            name: "oneof: swap",
            paths: &["*"],
            src: r#"{"oneofString":"src"}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofString":"src"}"#,
        },
        Case {
            name: "oneof: message swap",
            paths: &["*"],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofMessage1":{"string":"src"}}"#,
        },
    ]);
}

#[test]
fn wire_set_fields() {
    // Empty mask: copy only the fields populated on `src`, recursing into a set
    // singular message so its unmentioned fields are preserved.
    run(&[
        Case {
            name: "scalars",
            paths: &[],
            src: r#"{"double":111,"float":111}"#,
            dst: r#"{"double":222,"float":222,"string":"222","bytes":"3g=="}"#,
            expected: r#"{"double":111,"float":111,"string":"222","bytes":"3g=="}"#,
        },
        Case {
            name: "repeated",
            paths: &[],
            src: r#"{"repeatedDouble":[111]}"#,
            dst: r#"{"repeatedDouble":[222],"repeatedFloat":[222],"repeatedBool":[false],"repeatedString":["222"],"repeatedBytes":["3g=="]}"#,
            expected: r#"{"repeatedDouble":[111],"repeatedFloat":[222],"repeatedBool":[false],"repeatedString":["222"],"repeatedBytes":["3g=="]}"#,
        },
        Case {
            name: "nested",
            paths: &[],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{"message":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"message":{"string":"src","int64":"222"}}"#,
        },
        Case {
            name: "nested: dst nil",
            paths: &[],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{}"#,
            expected: r#"{"message":{"string":"src"}}"#,
        },
        Case {
            name: "maps",
            paths: &[],
            src: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
            dst: r#"{"mapStringString":{"dst-key":"dst-value"},"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
            expected: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
        },
        Case {
            name: "maps: dst nil",
            paths: &[],
            src: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
            dst: r#"{}"#,
            expected: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
        },
        Case {
            name: "oneof",
            paths: &[],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage1":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"oneofMessage1":{"string":"src","int64":"222"}}"#,
        },
        Case {
            name: "oneof: kind swap",
            paths: &[],
            src: r#"{"oneofString":"src"}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofString":"src"}"#,
        },
        Case {
            name: "oneof: message swap",
            paths: &[],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofMessage1":{"string":"src"}}"#,
        },
    ]);
}

#[test]
fn paths() {
    run(&[
        Case {
            name: "scalars",
            paths: &["double", "bytes"],
            src: r#"{"double":111,"float":111,"bytes":"bw=="}"#,
            dst: r#"{"double":222,"float":222,"string":"222","bytes":"3g=="}"#,
            expected: r#"{"double":111,"float":222,"bytes":"bw==","string":"222"}"#,
        },
        Case {
            name: "repeated scalar",
            paths: &["repeated_double", "repeated_string"],
            src: r#"{"repeatedDouble":[111],"repeatedFloat":[111]}"#,
            dst: r#"{"repeatedDouble":[222],"repeatedString":["222"],"repeatedBytes":["3g=="]}"#,
            expected: r#"{"repeatedDouble":[111],"repeatedBytes":["3g=="]}"#,
        },
        Case {
            name: "repeated message",
            paths: &["repeated_message"],
            src: r#"{"repeatedMessage":[{"string":"src"},{"int64":"111"}]}"#,
            dst: r#"{"repeatedMessage":[{"int64":"222"},{"string":"dst"}]}"#,
            expected: r#"{"repeatedMessage":[{"string":"src"},{"int64":"111"}]}"#,
        },
        Case {
            // Individual fields in a repeated message can not be updated.
            name: "repeated message: deep",
            paths: &["repeated_message.string"],
            src: r#"{"repeatedMessage":[{"string":"src"},{"int64":"111"}]}"#,
            dst: r#"{"repeatedMessage":[{"int64":"222"},{"string":"dst"}]}"#,
            expected: r#"{"repeatedMessage":[{"int64":"222"},{"string":"dst"}]}"#,
        },
        Case {
            name: "nested",
            paths: &["message"],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{"message":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"message":{"string":"src"}}"#,
        },
        Case {
            name: "nested: deep",
            paths: &["message.string"],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{"message":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"message":{"string":"src","int64":"222"}}"#,
        },
        Case {
            name: "nested: dst nil",
            paths: &["message"],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{}"#,
            expected: r#"{"message":{"string":"src"}}"#,
        },
        Case {
            name: "nested: deep, dst nil",
            paths: &["message.string"],
            src: r#"{"message":{"string":"src"}}"#,
            dst: r#"{}"#,
            expected: r#"{"message":{"string":"src"}}"#,
        },
        Case {
            name: "nested: deep, src nil",
            paths: &["message.string"],
            src: r#"{}"#,
            dst: r#"{"message":{"string":"src"}}"#,
            expected: r#"{"message":{}}"#,
        },
        Case {
            name: "maps",
            paths: &["map_string_string"],
            src: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
            dst: r#"{"mapStringString":{"dst-key":"dst-value"},"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
            expected: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
        },
        Case {
            // Individual entries in a map can not be updated.
            name: "maps: deep",
            paths: &["map_string_string.src1"],
            src: r#"{"mapStringString":{"src1":"src1-value","src2":"src2-value"}}"#,
            dst: r#"{"mapStringString":{"dst-key":"dst-value"}}"#,
            expected: r#"{"mapStringString":{"dst-key":"dst-value"}}"#,
        },
        Case {
            name: "maps: dst nil",
            paths: &["map_string_string"],
            src: r#"{"mapStringString":{"src-key":"src-value"},"mapStringMessage":{"src-key":{"string":"src-value"}}}"#,
            dst: r#"{}"#,
            expected: r#"{"mapStringString":{"src-key":"src-value"}}"#,
        },
        Case {
            name: "maps: src nil",
            paths: &["map_string_string"],
            src: r#"{}"#,
            dst: r#"{"mapStringString":{"dst-key":"dst-value"},"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
            expected: r#"{"mapStringMessage":{"dst-key":{"string":"dst-value"}}}"#,
        },
        Case {
            name: "oneof",
            paths: &["oneof_message1"],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage1":{"string":"dst","int64":"222"}}"#,
            expected: r#"{"oneofMessage1":{"string":"src"}}"#,
        },
        Case {
            name: "oneof: kind swap",
            paths: &["oneof_string"],
            src: r#"{"oneofString":"src"}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofString":"src"}"#,
        },
        Case {
            name: "oneof: kind swap src nil",
            paths: &["oneof_message2"],
            src: r#"{"oneofString":"src"}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{}"#,
        },
        Case {
            name: "oneof: deep",
            paths: &["oneof_message1.string"],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofMessage1":{"string":"src"}}"#,
        },
        Case {
            name: "oneof: deep src nil",
            paths: &["oneof_message2.string"],
            src: r#"{"oneofMessage1":{"string":"src"}}"#,
            dst: r#"{"oneofMessage2":{"string":"dst"}}"#,
            expected: r#"{"oneofMessage2":{}}"#,
        },
        Case {
            name: "message: src nil",
            paths: &["message"],
            src: r#"{}"#,
            dst: r#"{"message":{"int32":23}}"#,
            expected: r#"{}"#,
        },
    ]);
}
