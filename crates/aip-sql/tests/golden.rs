//! Golden tests: a representative filter (or hand-built [`Predicate`]) renders to
//! exact SQLite SQL plus an ordered list of binds. These pin the public surface
//! ADR-0008 fixes: the full AIP-160 operator set (`=` `!=` `<` `<=` `>` `>=`,
//! `AND` `OR` `NOT`, the has operator `:`), enum / timestamp / duration /
//! map-member type recovery, single-pass placeholder numbering, and precedence
//! parenthesization — plus the AIP-132 `order_by` → `ORDER BY` mapping and the
//! [`Query`] that unifies the WHERE, `ORDER BY`, and `LIMIT` / `OFFSET` tail into
//! one render.

use aip_filtering::{function, Declarations, Overload, Type};
use aip_ordering::OrderBy;
use aip_sql::{
    transpile_filter, transpile_order_by, Dialect, Order, Predicate, Query, Schema, Sqlite, Value,
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
}

/// Maps each filterable identifier onto its SQL column. The nested
/// `lat_lng.latitude` flattens to a `latitude` column; `labels` / `tags` are the
/// JSON map / list columns behind member access and the has operator.
fn schema() -> Schema {
    Schema::builder()
        .column("name", "name")
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

/// Parse an `order_by` string and transpile it against [`schema`], with `name`
/// as the key tie-break; returns just the SQL `ORDER BY` items.
fn order(order_by: &str) -> Vec<Order> {
    let parsed: OrderBy = order_by.parse().expect("order_by parses");
    let (_columns, orders) =
        transpile_order_by(&parsed, &schema(), &["name"]).expect("order_by transpiles");
    orders
}

#[test]
fn order_by_renders_multi_field_ascending_and_descending() {
    // A multi-field `order_by` maps each path to its column in priority order; the
    // implicit and explicit `asc` render `ASC`, `desc` renders `DESC`. The
    // always-on resource-name tie-break trails as `name ASC`, so the order is
    // total. An order-only `Query` is just the `ORDER BY` clause, with no binds.
    let items = order("display_name, size desc");
    assert_eq!(
        items,
        vec![
            Order::asc("display_name"),
            Order::desc("size"),
            Order::asc("name"),
        ],
    );
    let (sql, binds) = Query::new().order_by(items).render(&Sqlite);
    assert_eq!(sql, "ORDER BY display_name ASC, size DESC, name ASC");
    assert!(binds.is_empty());
}

#[test]
fn order_by_maps_nested_path_to_its_column() {
    // A `.`-nested path resolves through the schema to its flattened column, then
    // the resource-name tie-break trails it.
    let items = order("lat_lng.latitude desc");
    assert_eq!(items, vec![Order::desc("latitude"), Order::asc("name")]);
    assert_eq!(
        Query::new().order_by(items).render(&Sqlite).0,
        "ORDER BY latitude DESC, name ASC"
    );
}

#[test]
fn empty_order_by_yields_the_resource_name_tie_break() {
    // An empty `order_by` is not empty output: the always-on tie-break makes it
    // `name ASC`, so a consumer with no `order_by` still pages in a total, stable
    // order. This is the contract the example server leans on for `ListShipments`.
    let items = order("");
    assert_eq!(items, vec![Order::asc("name")]);
    assert_eq!(
        Query::new().order_by(items).render(&Sqlite).0,
        "ORDER BY name ASC"
    );
}

#[test]
fn order_by_on_name_is_not_duplicated_by_the_tie_break() {
    // When the user already sorts on `name`, the tie-break is skipped — in either
    // direction — so `name` never appears twice.
    assert_eq!(order("name desc"), vec![Order::desc("name")]);
    assert_eq!(
        order("display_name, name"),
        vec![Order::asc("display_name"), Order::asc("name")],
    );
}

#[test]
fn order_by_unmapped_path_is_rejected() {
    // A path the schema does not map is rejected, the same gate the filter
    // transpiler applies to an unmapped identifier.
    let parsed: OrderBy = "ghost".parse().expect("order_by parses");
    let err = transpile_order_by(&parsed, &schema(), &["name"]).expect_err("unmapped path");
    match err {
        aip_sql::Error::UnknownIdentifier(path) => assert_eq!(path, "ghost"),
        other => panic!("expected UnknownIdentifier, got {other:?}"),
    }
}

#[test]
fn tuple_gt_renders_a_row_value_keyset_seek() {
    // The cursor seek: a row-value comparison over the ordered seek columns, each
    // value bound in column order so the placeholders stay left-to-right.
    let seek = Predicate::tuple_gt(
        ["display_name", "shipper", "site"],
        [
            Value::Text("Oslo Dock".to_owned()),
            Value::Text("acme".to_owned()),
            Value::Text("dock-1".to_owned()),
        ],
    );
    let (sql, binds) = Sqlite.render(&seek);
    assert_eq!(sql, "(display_name, shipper, site) > (?1, ?2, ?3)");
    assert_eq!(
        binds,
        vec![
            Value::Text("Oslo Dock".to_owned()),
            Value::Text("acme".to_owned()),
            Value::Text("dock-1".to_owned()),
        ],
    );
}

#[test]
fn tuple_gt_composes_under_and_keeping_one_placeholder_pass() {
    // Composed with a scope equality and a soft-delete test, the whole predicate
    // numbers placeholders in one left-to-right pass; the seek is an atom (no
    // parens needed for precedence, only the row-value's own parens).
    let predicate = Predicate::all([
        Predicate::eq("shipper", Value::Text("acme".to_owned())),
        Predicate::is_null("delete_time"),
        Predicate::tuple_gt(
            ["display_name", "site"],
            [
                Value::Text("Oslo Dock".to_owned()),
                Value::Text("dock-1".to_owned()),
            ],
        ),
    ]);
    let (sql, binds) = Sqlite.render(&predicate);
    assert_eq!(
        sql,
        "shipper = ?1 AND delete_time IS NULL AND (display_name, site) > (?2, ?3)",
    );
    assert_eq!(binds.len(), 3);
}

#[test]
fn transpile_order_by_returns_cursor_columns_with_types_and_key_tie_break() {
    // Against a declared schema, the seek list pairs each ordered column with its
    // declared Type and appends the key columns (uniformly text) as the tie-break.
    let declarations = Declarations::builder()
        .ident("display_name", Type::String)
        .ident("size", Type::Int)
        .build();
    let schema = Schema::for_declarations(&declarations).build();

    let parsed: OrderBy = "display_name, size desc".parse().expect("order_by parses");
    let (columns, orders) =
        transpile_order_by(&parsed, &schema, &["shipper", "site"]).expect("transpiles");

    assert_eq!(
        columns,
        vec![
            ("display_name".to_owned(), Type::String),
            ("size".to_owned(), Type::Int),
            ("shipper".to_owned(), Type::String),
            ("site".to_owned(), Type::String),
        ],
    );
    assert_eq!(
        orders,
        vec![
            Order::asc("display_name"),
            Order::desc("size"),
            Order::asc("shipper"),
            Order::asc("site"),
        ],
    );
}

// ----- The unified `Query`: WHERE + ORDER BY + LIMIT/OFFSET in one render -----

#[test]
fn query_unifies_where_order_and_page_into_one_tail() {
    // The headline: one `render` call emits `WHERE … ORDER BY … LIMIT … OFFSET`
    // plus only the binds the WHERE produced — the filter and `order_by` halves no
    // longer render through two separate mechanisms. The WHERE's placeholders keep
    // their single left-to-right numbering (`?1`, `?2`); the `ORDER BY` columns and
    // the page integers add no binds.
    let predicate = transpile_filter(
        &aip_filtering::check(r#"region = "west" AND size > 3"#, &declarations())
            .expect("filter checks"),
        &declarations(),
        &schema(),
    )
    .expect("filter transpiles");
    let (sql, binds) = Query::new()
        .filter(predicate)
        .order_by([Order::asc("display_name"), Order::asc("name")])
        .limit(51)
        .offset(100)
        .render(&Sqlite);
    assert_eq!(
        sql,
        "WHERE region = ?1 AND size > ?2 ORDER BY display_name ASC, name ASC LIMIT 51 OFFSET 100",
    );
    assert_eq!(binds, vec![Value::Text("west".to_string()), Value::Int(3)]);
}

#[test]
fn query_with_only_a_filter_is_just_a_where() {
    // A filter-only `Query` is a bare `WHERE`, carrying the predicate's binds.
    let (sql, binds) = Query::new()
        .filter(Predicate::eq("size", Value::Int(3)))
        .render(&Sqlite);
    assert_eq!(sql, "WHERE size = ?1");
    assert_eq!(binds, vec![Value::Int(3)]);
}

#[test]
fn query_omits_absent_clauses() {
    // Only the parts that are set render: with no page tail, just WHERE + ORDER BY.
    let (sql, binds) = Query::new()
        .filter(Predicate::eq("region", Value::Text("west".to_string())))
        .order_by([Order::desc("size")])
        .render(&Sqlite);
    assert_eq!(sql, "WHERE region = ?1 ORDER BY size DESC");
    assert_eq!(binds, vec![Value::Text("west".to_string())]);
}

#[test]
fn query_renders_limit_and_offset_directly() {
    // The resolved page size and offset render as a decimal `LIMIT` / `OFFSET` tail
    // — no binds, since neither is free-form text.
    assert_eq!(
        Query::new().limit(50).offset(100).render(&Sqlite).0,
        "LIMIT 50 OFFSET 100"
    );
    assert_eq!(
        Query::new().limit(10).offset(0).render(&Sqlite).0,
        "LIMIT 10 OFFSET 0"
    );
}

#[test]
fn empty_query_renders_nothing() {
    // An all-empty `Query` renders to `("", [])`.
    let (sql, binds) = Query::new().render(&Sqlite);
    assert_eq!(sql, "");
    assert!(binds.is_empty());
}

// ----- The public `Predicate` builder surface for server-side composition -----

#[test]
fn scope_to_parent_binds_an_escaped_prefix() {
    // An AIP parent scope is a `LIKE` prefix keeping the rows under `parent`: the
    // parent plus the child wildcard `/%`, bound under an explicit `ESCAPE` —
    // never interpolated.
    let (sql, binds) = Sqlite.render(&Predicate::scope_to_parent("name", "shippers/acme"));
    assert_eq!(sql, r"name LIKE ?1 ESCAPE '\'");
    assert_eq!(binds, vec![Value::Text("shippers/acme/%".to_string())]);
}

#[test]
fn scope_to_parent_matches_metacharacters_literally() {
    // A parent containing the `LIKE` wildcards `%` / `_` (or the escape char `\`)
    // has them escaped in the bound pattern, so it matches literally — a parent
    // can't smuggle a wildcard into the scope.
    let (sql, binds) = Sqlite.render(&Predicate::scope_to_parent("name", r"tenants/a%b_c"));
    assert_eq!(sql, r"name LIKE ?1 ESCAPE '\'");
    assert_eq!(binds, vec![Value::Text(r"tenants/a\%b\_c/%".to_string())]);
}

#[test]
fn is_null_renders_a_null_test() {
    // The soft-delete predicate a server composes with a user filter; it binds
    // nothing.
    let (sql, binds) = Sqlite.render(&Predicate::is_null("delete_time"));
    assert_eq!(sql, "delete_time IS NULL");
    assert!(binds.is_empty());
}

#[test]
fn raw_fragment_is_verbatim_at_the_root_and_parenthesized_as_a_child() {
    // A raw fragment is emitted as-is at the root, and — because its internals are
    // opaque — always parenthesized when composed under a combinator. It carries
    // no binds, so the user filter beside it still numbers from `?1`.
    assert_eq!(
        Sqlite.render(&Predicate::raw("archived = 0")).0,
        "archived = 0"
    );

    let composed = Predicate::all([
        Predicate::raw("archived = 0"),
        Predicate::eq("region", Value::Text("west".to_string())),
    ]);
    let (sql, binds) = Sqlite.render(&composed);
    assert_eq!(sql, "(archived = 0) AND region = ?1");
    assert_eq!(binds, vec![Value::Text("west".to_string())]);
}

#[test]
fn server_composes_scope_tenancy_soft_delete_and_user_filter() {
    // The headline composition #43 exists for: a server folds its own predicates
    // — a parent scope, a tenancy `eq`, a soft-delete `IS NULL` — and the user's
    // (transpiled) filter into one fragment that owns precedence and one coherent
    // left-to-right placeholder numbering. The user filter's `OR` is parenthesized
    // under the surrounding `AND`, and the bind-free soft-delete leaf consumes no
    // placeholder, so the numbering steps 1 → 2 → 3 → 4 across the fragments that
    // do bind.
    let user_filter = transpile_filter(
        &aip_filtering::check(r#"region = "west" OR region = "east""#, &declarations())
            .expect("filter checks"),
        &declarations(),
        &schema(),
    )
    .expect("filter transpiles");

    let composed = Predicate::all([
        Predicate::scope_to_parent("name", "shippers/acme"),
        Predicate::eq("tenant_id", Value::Int(7)),
        Predicate::is_null("delete_time"),
        user_filter,
    ]);

    let (sql, binds) = Sqlite.render(&composed);
    assert_eq!(
        sql,
        r"name LIKE ?1 ESCAPE '\' AND tenant_id = ?2 AND delete_time IS NULL AND (region = ?3 OR region = ?4)",
    );
    assert_eq!(
        binds,
        vec![
            Value::Text("shippers/acme/%".to_string()),
            Value::Int(7),
            Value::Text("west".to_string()),
            Value::Text("east".to_string()),
        ],
    );
}
