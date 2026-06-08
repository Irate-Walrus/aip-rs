//! Golden tests: a representative filter (or hand-built [`Predicate`]) renders to
//! exact SQLite SQL plus an ordered list of binds. These pin the public surface
//! ADR-0008 fixes: the full AIP-160 operator set (`=` `!=` `<` `<=` `>` `>=`,
//! `AND` `OR` `NOT`, the has operator `:`), enum / timestamp / duration /
//! map-member type recovery, single-pass placeholder numbering, and precedence
//! parenthesization — plus the AIP-132 `order_by` → `ORDER BY` mapping and the
//! `LIMIT` / `OFFSET` tail.

use aip_filtering::{function, Declarations, Overload, Type};
use aip_ordering::OrderBy;
use aip_sql::{
    render_limit_offset, render_order_by, transpile_filter, transpile_order_by, Dialect, Order,
    Predicate, Schema, Sqlite, Value,
};

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
        .ident("tags", Type::List(Box::new(Type::String)))
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
/// `lat_lng.latitude` flattens to a `latitude` column; `labels` / `tags` are the
/// JSON map / list columns behind member access and the has operator.
fn schema() -> Schema {
    Schema::builder()
        .column("display_name", "display_name")
        .column("region", "region")
        .column("size", "size")
        .column("lat_lng.latitude", "latitude")
        .column("create_time", "create_time")
        .column("ttl", "ttl")
        .column("labels", "labels")
        .column("tags", "tags")
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
fn has_on_string_renders_escaped_substring_like() {
    // `:` on a string column is a substring match: a `LIKE` whose pattern wraps
    // the value in `%…%`, bound under an explicit `ESCAPE` — never interpolated.
    let (sql, binds) = render(r#"display_name : "Alpha""#);
    assert_eq!(sql, r"display_name LIKE ?1 ESCAPE '\'");
    assert_eq!(binds, vec![Value::Text("%Alpha%".to_string())]);
}

#[test]
fn has_on_string_escapes_like_metacharacters() {
    // The `LIKE` wildcards `%` / `_` and the escape char `\` in the value are
    // each escaped so they match literally — user input can't act as a wildcard.
    // (The filter literal `\\` lexes to a single backslash in the value.)
    let (sql, binds) = render(r#"display_name : "a%b_c\\d""#);
    assert_eq!(sql, r"display_name LIKE ?1 ESCAPE '\'");
    assert_eq!(binds, vec![Value::Text(r"%a\%b\_c\\d%".to_string())]);
}

#[test]
fn has_on_map_renders_key_presence() {
    // `:` on a `map<string,string>` tests key presence via `json_each`; the key
    // is bound.
    let (sql, binds) = render("labels:env");
    assert_eq!(
        sql,
        "EXISTS (SELECT 1 FROM json_each(labels) WHERE key = ?1)"
    );
    assert_eq!(binds, vec![Value::Text("env".to_string())]);
}

#[test]
fn has_on_list_renders_membership() {
    // `:` on a `list<string>` tests element presence via `json_each`; the value
    // is bound.
    let (sql, binds) = render("tags:urgent");
    assert_eq!(
        sql,
        "EXISTS (SELECT 1 FROM json_each(tags) WHERE value = ?1)"
    );
    assert_eq!(binds, vec![Value::Text("urgent".to_string())]);
}

#[test]
fn has_on_timestamp_renders_presence() {
    // `:` on a timestamp is presence-only (`field:*`): a `NULL` test that binds
    // nothing. The checker has already restricted the argument to `*`.
    let (sql, binds) = render("create_time:*");
    assert_eq!(sql, "create_time IS NOT NULL");
    assert!(binds.is_empty());
}

#[test]
fn has_leaf_numbers_in_step_and_negates_without_parens() {
    // A has leaf binds as tightly as a comparison: it numbers left-to-right
    // alongside other leaves, and `NOT` wraps it without parentheses.
    let (sql, binds) = render(r#"display_name = "a" AND labels:env"#);
    assert_eq!(
        sql,
        "display_name = ?1 AND EXISTS (SELECT 1 FROM json_each(labels) WHERE key = ?2)"
    );
    assert_eq!(
        binds,
        vec![Value::Text("a".to_string()), Value::Text("env".to_string()),],
    );

    assert_eq!(render("NOT create_time:*").0, "NOT create_time IS NOT NULL");
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

/// Parse an `order_by` string and transpile it against [`schema`].
fn order(order_by: &str) -> Vec<Order> {
    let parsed: OrderBy = order_by.parse().expect("order_by parses");
    transpile_order_by(&parsed, &schema()).expect("order_by transpiles")
}

#[test]
fn order_by_renders_multi_field_ascending_and_descending() {
    // A multi-field `order_by` maps each path to its column in priority order; the
    // implicit and explicit `asc` render `ASC`, `desc` renders `DESC`.
    let items = order("display_name, size desc");
    assert_eq!(items, vec![Order::asc("display_name"), Order::desc("size")]);
    assert_eq!(render_order_by(&items), "display_name ASC, size DESC");
}

#[test]
fn order_by_maps_nested_path_to_its_column() {
    // A `.`-nested path resolves through the schema to its flattened column.
    let items = order("lat_lng.latitude desc");
    assert_eq!(items, vec![Order::desc("latitude")]);
    assert_eq!(render_order_by(&items), "latitude DESC");
}

#[test]
fn empty_order_by_renders_nothing() {
    // An empty `order_by` is valid and yields no items, so the caller emits no
    // `ORDER BY` clause.
    let items = order("");
    assert!(items.is_empty());
    assert_eq!(render_order_by(&items), "");
}

#[test]
fn order_by_unmapped_path_is_rejected() {
    // A path the schema does not map is rejected, the same gate the filter
    // transpiler applies to an unmapped identifier.
    let parsed: OrderBy = "ghost".parse().expect("order_by parses");
    let err = transpile_order_by(&parsed, &schema()).expect_err("unmapped path");
    match err {
        aip_sql::Error::UnknownIdentifier(path) => assert_eq!(path, "ghost"),
        other => panic!("expected UnknownIdentifier, got {other:?}"),
    }
}

#[test]
fn limit_offset_renders_from_page_size_and_offset() {
    // The resolved page size and offset page-token offset render as a decimal
    // `LIMIT` / `OFFSET` tail — no binds, since neither is free-form text.
    assert_eq!(render_limit_offset(50, 100), "LIMIT 50 OFFSET 100");
    assert_eq!(render_limit_offset(10, 0), "LIMIT 10 OFFSET 0");
}
