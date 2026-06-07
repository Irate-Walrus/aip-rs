//! Golden tests: a representative filter (or hand-built [`Predicate`]) renders to
//! exact SQLite SQL plus an ordered list of binds. These pin the public surface
//! this tracer-bullet slice fixes (ADR-0008): `=`/`AND` transpilation, single-pass
//! placeholder numbering, and precedence parenthesization.

use aip_filtering::{Declarations, Type};
use aip_sql::{transpile_filter, Dialect, Predicate, Schema, Sqlite, Value};

/// Declarations accepting `=`/`AND` over two string columns, plus the full
/// standard operator set — so the *transpiler* (not the checker) is the gate that
/// rejects anything beyond this slice.
fn declarations() -> Declarations {
    Declarations::builder()
        .standard_functions()
        .ident("display_name", Type::String)
        .ident("region", Type::String)
        .build()
        .expect("declarations build")
}

/// Maps both filterable identifiers onto identically-named SQL columns.
fn schema() -> Schema {
    Schema::builder()
        .column("display_name", "display_name")
        .column("region", "region")
        .build()
}

/// Parse + type-check `filter`, transpile against [`schema`], and render to
/// SQLite `(sql, binds)`.
fn render(filter: &str) -> (String, Vec<Value>) {
    let checked = aip_filtering::check(filter, &declarations()).expect("filter checks");
    let predicate = transpile_filter(&checked, &schema()).expect("filter transpiles");
    Sqlite.render(&predicate)
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
fn or_inside_and_is_parenthesized() {
    // The reason `Predicate` exists (ADR-0008): an OR composed under an AND must
    // be parenthesized so it doesn't silently re-bind. Placeholders still run
    // left-to-right across the nesting.
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
fn unsupported_operator_is_rejected() {
    // `!=` type-checks but is beyond this slice's transpiler.
    let checked = aip_filtering::check(r#"display_name != "Alpha""#, &declarations()).unwrap();
    let err = transpile_filter(&checked, &schema()).expect_err("`!=` is unsupported");
    assert!(matches!(err, aip_sql::Error::Unsupported(_)), "got {err:?}");
}

#[test]
fn unknown_identifier_is_rejected() {
    // `region` is declared (so it checks) but absent from a name-only schema.
    let checked = aip_filtering::check(r#"region = "west""#, &declarations()).unwrap();
    let name_only = Schema::builder()
        .column("display_name", "display_name")
        .build();
    let err = transpile_filter(&checked, &name_only).expect_err("unmapped column");
    match err {
        aip_sql::Error::UnknownIdentifier(ident) => assert_eq!(ident, "region"),
        other => panic!("expected UnknownIdentifier, got {other:?}"),
    }
}
