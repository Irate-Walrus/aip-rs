//! Golden tests: a representative filter (or hand-built [`Predicate`]) renders to
//! exact SQLite SQL plus an ordered list of binds. These pin the public surface
//! ADR-0008 fixes: the full AIP-160 operator set (`=` `!=` `<` `<=` `>` `>=`,
//! `AND` `OR` `NOT`), enum / timestamp / duration / map-member type recovery,
//! single-pass placeholder numbering, and precedence parenthesization.

use aip_filtering::{function, Declarations, Overload, Type};
use aip_sql::{transpile_filter, Dialect, Predicate, Schema, Sqlite, Value};

/// The enum type both the `category` field and its bare value names share.
fn enum_type() -> Type {
    Type::Enum("example.Category".to_string())
}

/// Declarations covering one identifier of each filterable shape, on top of the
/// full standard operator set — so the *transpiler* (not the checker) is the gate
/// for anything it can't lower. The enum is declared by hand (a value name, its
/// field, and the `=`/`!=` overload) so this crate stays reflection-free; the
/// reflective `enum_ident` path is exercised by the example and by
/// aip-filtering's own tests.
fn declarations() -> Declarations {
    Declarations::builder()
        .standard_functions()
        .ident("display_name", Type::String)
        .ident("region", Type::String)
        .ident("size", Type::Int)
        .ident("lat_lng.latitude", Type::Double)
        .ident("create_time", Type::Timestamp)
        .ident("ttl", Type::Duration)
        .ident(
            "labels",
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
        )
        .ident("category", enum_type())
        .ident("ENUM_ONE", enum_type())
        .ident("ENUM_TWO", enum_type())
        .function(
            function::EQUALS,
            vec![Overload::new(Type::Bool, vec![enum_type(), enum_type()])],
        )
        .function(
            function::NOT_EQUALS,
            vec![Overload::new(Type::Bool, vec![enum_type(), enum_type()])],
        )
        .build()
        .expect("declarations build")
}

/// Maps each filterable identifier onto its SQL column. The nested
/// `lat_lng.latitude` flattens to a `latitude` column; `labels` is the JSON map
/// column behind member access.
fn schema() -> Schema {
    Schema::builder()
        .column("display_name", "display_name")
        .column("region", "region")
        .column("size", "size")
        .column("lat_lng.latitude", "latitude")
        .column("create_time", "create_time")
        .column("ttl", "ttl")
        .column("labels", "labels")
        .column("category", "category")
        .build()
}

/// Parse + type-check `filter`, transpile against [`declarations`] / [`schema`],
/// and render to SQLite `(sql, binds)`.
fn render(filter: &str) -> (String, Vec<Value>) {
    let checked = aip_filtering::check(filter, &declarations()).expect("filter checks");
    let predicate =
        transpile_filter(&checked, &declarations(), &schema()).expect("filter transpiles");
    Sqlite.render(&predicate)
}

/// Transpile `filter` and return the [`Error`](aip_sql::Error), expecting it not
/// to transpile.
fn transpile_err(filter: &str) -> aip_sql::Error {
    let checked = aip_filtering::check(filter, &declarations()).expect("filter checks");
    transpile_filter(&checked, &declarations(), &schema()).expect_err("filter should not transpile")
}

#[test]
fn single_equality_binds_the_literal() {
    let (sql, binds) = render(r#"display_name = "Alpha""#);
    assert_eq!(sql, "display_name = ?1");
    assert_eq!(binds, vec![Value::Text("Alpha".to_string())]);
}

#[test]
fn conjunction_numbers_placeholders_left_to_right() {
    let (sql, binds) = render(r#"display_name = "Alpha" AND region = "west""#);
    assert_eq!(sql, "display_name = ?1 AND region = ?2");
    assert_eq!(
        binds,
        vec![
            Value::Text("Alpha".to_string()),
            Value::Text("west".to_string()),
        ],
    );
}

#[test]
fn nested_conjunction_stays_flat_and_in_order() {
    // `a AND b AND c` parses left-associatively to AND(AND(a, b), c); AND is
    // associative, so it renders without redundant parens and numbers 1..=3.
    let (sql, binds) = render(r#"display_name = "a" AND region = "b" AND display_name = "c""#);
    assert_eq!(
        sql,
        "display_name = ?1 AND region = ?2 AND display_name = ?3"
    );
    assert_eq!(
        binds,
        vec![
            Value::Text("a".to_string()),
            Value::Text("b".to_string()),
            Value::Text("c".to_string()),
        ],
    );
}

#[test]
fn inequality_and_ordering_operators_render_their_spelling() {
    // `!=` `<` `<=` `>` `>=` each lower to a comparison leaf with that spelling.
    for (filter, expected) in [
        (r#"display_name != "x""#, "display_name != ?1"),
        ("size < 1", "size < ?1"),
        ("size <= 2", "size <= ?1"),
        ("size > 3", "size > ?1"),
        ("size >= 4", "size >= ?1"),
    ] {
        assert_eq!(render(filter).0, expected, "filter {filter:?}");
    }
}

#[test]
fn disjunction_lowers_to_or() {
    let (sql, binds) = render(r#"region = "west" OR region = "east""#);
    assert_eq!(sql, "region = ?1 OR region = ?2");
    assert_eq!(
        binds,
        vec![
            Value::Text("west".to_string()),
            Value::Text("east".to_string()),
        ],
    );
}

#[test]
fn negation_lowers_to_not() {
    // `NOT` binds looser than the comparison it negates, so no parens are needed.
    let (sql, binds) = render(r#"NOT display_name = "x""#);
    assert_eq!(sql, "NOT display_name = ?1");
    assert_eq!(binds, vec![Value::Text("x".to_string())]);
}

#[test]
fn or_under_and_is_parenthesized_from_a_real_filter() {
    // The footgun `Predicate` exists to prevent: an `OR` composed under an `AND`
    // is parenthesized, and placeholders still run left-to-right across nesting.
    let (sql, binds) = render(r#"display_name = "a" AND (region = "b" OR region = "c")"#);
    assert_eq!(sql, "display_name = ?1 AND (region = ?2 OR region = ?3)");
    assert_eq!(
        binds,
        vec![
            Value::Text("a".to_string()),
            Value::Text("b".to_string()),
            Value::Text("c".to_string()),
        ],
    );
}

#[test]
fn numeric_literals_bind_by_type() {
    // An integer column binds an `Int`; a double column (reached through the
    // nested `lat_lng.latitude` path) binds a `Double`.
    let (sql, binds) = render("size >= 10");
    assert_eq!(sql, "size >= ?1");
    assert_eq!(binds, vec![Value::Int(10)]);

    let (sql, binds) = render("lat_lng.latitude > 30.5");
    assert_eq!(sql, "latitude > ?1");
    assert_eq!(binds, vec![Value::Double(30.5)]);
}

#[test]
fn timestamp_binds_as_rfc3339_text() {
    // A timestamp field compares against an RFC3339 string literal directly, or
    // via the `timestamp(...)` constructor; both bind the same text.
    let (sql, binds) = render(r#"create_time > "2024-01-01T00:00:00Z""#);
    assert_eq!(sql, "create_time > ?1");
    assert_eq!(binds, vec![Value::Text("2024-01-01T00:00:00Z".to_string())]);

    let (sql, binds) = render(r#"create_time = timestamp("2024-01-01T00:00:00Z")"#);
    assert_eq!(sql, "create_time = ?1");
    assert_eq!(binds, vec![Value::Text("2024-01-01T00:00:00Z".to_string())]);
}

#[test]
fn duration_binds_as_total_seconds() {
    // A `duration(...)` literal is normalized to seconds so it compares
    // numerically against a seconds-valued column.
    let (sql, binds) = render(r#"ttl > duration("3600s")"#);
    assert_eq!(sql, "ttl > ?1");
    assert_eq!(binds, vec![Value::Double(3600.0)]);

    assert_eq!(
        render(r#"ttl <= duration("1.5s")"#).1,
        vec![Value::Double(1.5)]
    );
}

#[test]
fn malformed_duration_is_rejected() {
    // The checker accepts any string for `duration(...)`; a non-seconds value is
    // caught here.
    match transpile_err(r#"ttl > duration("oops")"#) {
        aip_sql::Error::InvalidDuration(literal) => assert_eq!(literal, "oops"),
        other => panic!("expected InvalidDuration, got {other:?}"),
    }
}

#[test]
fn enum_comparison_binds_the_value_name() {
    // The enum's bare value name is bound as text (its name), on either side of
    // the operator.
    let (sql, binds) = render("category = ENUM_ONE");
    assert_eq!(sql, "category = ?1");
    assert_eq!(binds, vec![Value::Text("ENUM_ONE".to_string())]);

    let (sql, binds) = render("ENUM_TWO != category");
    assert_eq!(sql, "category != ?1");
    assert_eq!(binds, vec![Value::Text("ENUM_TWO".to_string())]);
}

#[test]
fn map_member_selection_binds_key_then_value() {
    // `labels.env` reads a value from the JSON map column with `->>`; the key is
    // bound (filter input) and numbered before the comparison value.
    let (sql, binds) = render(r#"labels.env = "prod""#);
    assert_eq!(sql, "labels ->> ?1 = ?2");
    assert_eq!(
        binds,
        vec![
            Value::Text("env".to_string()),
            Value::Text("prod".to_string()),
        ],
    );
}

#[test]
fn or_inside_and_is_parenthesized() {
    // The same parenthesization, asserted on a hand-built `Predicate` to pin the
    // builder/renderer contract independent of the parser.
    let predicate = Predicate::all([
        Predicate::any([
            Predicate::eq("a", Value::Int(1)),
            Predicate::eq("b", Value::Int(2)),
        ]),
        Predicate::eq("c", Value::Int(3)),
    ]);
    let (sql, binds) = Sqlite.render(&predicate);
    assert_eq!(sql, "(a = ?1 OR b = ?2) AND c = ?3");
    assert_eq!(binds, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
}

#[test]
fn not_wraps_a_lower_precedence_child() {
    // `NOT` binds tighter than `OR`, so the disjunction it negates is wrapped;
    // a negated comparison is not (comparison binds tighter than `NOT`).
    let wrapped = Predicate::not(Predicate::any([
        Predicate::eq("a", Value::Bool(true)),
        Predicate::eq("b", Value::Null),
    ]));
    let (sql, binds) = Sqlite.render(&wrapped);
    assert_eq!(sql, "NOT (a = ?1 OR b = ?2)");
    assert_eq!(binds, vec![Value::Bool(true), Value::Null]);

    let bare = Predicate::not(Predicate::eq("a", Value::Int(7)));
    assert_eq!(Sqlite.render(&bare).0, "NOT a = ?1");
}

#[test]
fn literal_is_bound_on_either_side_of_equals() {
    // The literal may sit on the left of `=`; it is still bound, never spliced.
    let (sql, binds) = render(r#""Alpha" = display_name"#);
    assert_eq!(sql, "display_name = ?1");
    assert_eq!(binds, vec![Value::Text("Alpha".to_string())]);
}

#[test]
fn has_operator_is_unsupported() {
    // The has operator `:` type-checks (it is in the standard set) but is the
    // next slice (#41), so the transpiler is the gate that rejects it.
    let err = transpile_err(r#"display_name : "Alpha""#);
    assert!(matches!(err, aip_sql::Error::Unsupported(_)), "got {err:?}");
}

#[test]
fn comparison_between_two_columns_is_unsupported() {
    // Both sides resolve to columns, so there is no value to bind — out of scope
    // for this slice.
    let err = transpile_err("display_name < region");
    assert!(matches!(err, aip_sql::Error::Unsupported(_)), "got {err:?}");
}

#[test]
fn unknown_identifier_is_rejected() {
    // `region` is declared (so it checks) but absent from a name-only schema; it
    // is a scalar identifier, so it is reported as an unmapped column rather than
    // mistaken for an enum value.
    let checked = aip_filtering::check(r#"region = "west""#, &declarations()).unwrap();
    let name_only = Schema::builder()
        .column("display_name", "display_name")
        .build();
    let err = transpile_filter(&checked, &declarations(), &name_only).expect_err("unmapped column");
    match err {
        aip_sql::Error::UnknownIdentifier(ident) => assert_eq!(ident, "region"),
        other => panic!("expected UnknownIdentifier, got {other:?}"),
    }
}
