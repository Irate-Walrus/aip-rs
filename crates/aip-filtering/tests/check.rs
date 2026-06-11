//! Table tests for the public [`check`]: a filter is type-checked against a
//! [`Declarations`] allowlist — identifiers and dotted members resolve, calls
//! resolve a declared overload, and the whole filter must be a `bool`.
//!
//! Ported from `aip-go`'s `checker_test.go`, excluding the enum and the
//! timestamp/duration *comparison* overload cases (which land with their own
//! slices); the `:` (has) operator, including its timestamp overload, is here.

use aip_filtering::Type::{self, Bool, Double, Int, String, Timestamp};
use aip_filtering::{check, Declarations, DeclarationsBuilder, Error, Overload};

/// Build declarations on top of the standard function set via `build`.
fn decls(build: impl FnOnce(DeclarationsBuilder) -> DeclarationsBuilder) -> Declarations {
    build(Declarations::builder().standard_functions()).build()
}

fn map(key: Type, value: Type) -> Type {
    Type::Map(Box::new(key), Box::new(value))
}

fn list(element: Type) -> Type {
    Type::List(Box::new(element))
}

/// A custom function with a single overload over `params` returning `result`.
fn func(
    builder: DeclarationsBuilder,
    name: &str,
    result: Type,
    params: Vec<Type>,
) -> DeclarationsBuilder {
    builder.function(name, vec![Overload::new(result, params)])
}

#[test]
fn accepts_well_typed_filters() {
    let cases: Vec<(&str, Declarations)> = vec![
        // A bare bool identifier is a valid (bool-typed) filter.
        ("a", decls(|b| b.ident("a", Bool))),
        ("prod", decls(|b| b.ident("prod", Bool))),
        (
            r#"author = "Karin Boye" AND NOT read"#,
            decls(|b| b.ident("author", String).ident("read", Bool)),
        ),
        ("a < 10 OR a >= 100", decls(|b| b.ident("a", Int))),
        (
            "NOT (a OR b)",
            decls(|b| b.ident("a", Bool).ident("b", Bool)),
        ),
        // `com.google` resolves by selecting the value type of the `com` map.
        (
            "package=com.google",
            decls(|b| b.ident("package", String).ident("com", map(String, String))),
        ),
        (r#"msg != 'hello'"#, decls(|b| b.ident("msg", String))),
        ("1 > 0", decls(|b| b)),
        ("2.5 >= 2.4", decls(|b| b)),
        ("foo >= -2.4", decls(|b| b.ident("foo", Double))),
        // A double field compared to an int literal (the double/int overload).
        ("foo = 3", decls(|b| b.ident("foo", Double))),
        ("foo >= 3", decls(|b| b.ident("foo", Double))),
        ("foo <= 3", decls(|b| b.ident("foo", Double))),
        ("foo > 3", decls(|b| b.ident("foo", Double))),
        ("foo < 3", decls(|b| b.ident("foo", Double))),
        ("foo >= (-2.4)", decls(|b| b.ident("foo", Double))),
        ("-2.5 >= -2.4", decls(|b| b)),
        (
            r#"display_name = "Acme""#,
            decls(|b| b.ident("display_name", String)),
        ),
        // A dotted member resolves against its declared full name.
        (
            r#"book.author = "Boye""#,
            decls(|b| b.ident("book.author", String)),
        ),
        // Custom functions resolve their declared overloads.
        (
            "regex(m.key, '^.*prod.*$')",
            decls(|b| {
                func(
                    b.ident("m", map(String, String)),
                    "regex",
                    Bool,
                    vec![String, String],
                )
            }),
        ),
        (
            "math.mem('30mb')",
            decls(|b| func(b, "math.mem", Bool, vec![String])),
        ),
        (
            "(endsWith(msg, 'world') AND retries < 10)",
            decls(|b| {
                func(
                    b.ident("msg", String).ident("retries", Int),
                    "endsWith",
                    Bool,
                    vec![String, String],
                )
            }),
        ),
    ];
    for (filter, declarations) in cases {
        check(filter, &declarations)
            .unwrap_or_else(|e| panic!("filter {filter:?} should check: {e}"));
    }
}

#[test]
fn rejects_undeclared_functions() {
    // The implicit-AND `FUZZY` is intentionally not a standard function, so a
    // space-separated sequence is rejected unless the caller declares it.
    let cases: Vec<(&str, Declarations)> = vec![
        (
            "New York Giants",
            decls(|b| {
                b.ident("New", Bool)
                    .ident("York", Bool)
                    .ident("Giants", Bool)
            }),
        ),
        (
            "New York Giants OR Yankees",
            decls(|b| {
                b.ident("New", Bool)
                    .ident("York", Bool)
                    .ident("Giants", Bool)
                    .ident("Yankees", Bool)
            }),
        ),
        (
            "(msg.endsWith('world') AND retries < 10)",
            decls(|b| b.ident("retries", Int)),
        ),
    ];
    for (filter, declarations) in cases {
        match check(filter, &declarations) {
            Err(Error::Type(message)) => assert!(
                message.contains("undeclared function"),
                "filter {filter:?}: message {message:?} lacks 'undeclared function'"
            ),
            other => {
                panic!("filter {filter:?}: expected an undeclared-function error, got {other:?}")
            }
        }
    }
}

#[test]
fn rejects_undeclared_identifiers() {
    // (filter, declarations, the identifier reported as undeclared)
    let cases: Vec<(&str, Declarations, &str)> = vec![
        (r#"name = "x""#, decls(|b| b), "name"),
        // An undeclared dotted member resolves down to its operand, so the
        // undeclared *root* identifier is what's reported (as in aip-go).
        (r#"book.author = "x""#, decls(|b| b), "book"),
        // Structs aren't supported: the operand identifier is undeclared.
        (
            "yesterday < request.time",
            decls(|b| b.ident("yesterday", Type::Timestamp)),
            "request",
        ),
        (
            "experiment.rollout <= cohort(request.user)",
            decls(|b| func(b, "cohort", Double, vec![String])),
            "experiment",
        ),
        ("expr.type_map.1.type", decls(|b| b), "expr"),
    ];
    for (filter, declarations, name) in cases {
        match check(filter, &declarations) {
            Err(Error::UndeclaredIdent(reported)) => {
                assert_eq!(reported, name, "filter {filter:?}: wrong identifier")
            }
            other => {
                panic!("filter {filter:?}: expected an undeclared-identifier error, got {other:?}")
            }
        }
    }
}

#[test]
fn rejects_type_mismatches() {
    let cases: Vec<(&str, Declarations)> = vec![
        // String field compared to an int literal: no matching overload.
        ("count = 3", decls(|b| b.ident("count", String))),
        // Int field ordered against a string literal.
        (r#"age > "x""#, decls(|b| b.ident("age", Int))),
        // The double/int overload is one-directional (no int-then-double).
        ("count = 3.0", decls(|b| b.ident("count", Int))),
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
fn rejects_non_bool_results() {
    let cases: Vec<(&str, Declarations)> = vec![
        ("-30", decls(|b| b)),
        ("name", decls(|b| b.ident("name", String))),
    ];
    for (filter, declarations) in cases {
        match check(filter, &declarations) {
            Err(Error::Type(message)) => assert!(
                message.contains("non-bool"),
                "filter {filter:?}: message {message:?} lacks 'non-bool'"
            ),
            other => panic!("filter {filter:?}: expected a non-bool type error, got {other:?}"),
        }
    }
}

#[test]
fn surfaces_syntax_errors() {
    for filter in ["<", "(-2.5) >= -2.4", r#"a = "foo"#] {
        let declarations = decls(|b| b);
        match check(filter, &declarations) {
            Err(Error::Syntax { .. }) => {}
            other => panic!("filter {filter:?}: expected a syntax error, got {other:?}"),
        }
    }
}

#[test]
fn function_builder_appends_overloads() {
    // Declaring the same function name twice appends overloads rather than
    // replacing, so both argument shapes resolve. This is how the later slices
    // add timestamp/enum overloads onto the standard `=` / `<` names.
    let declarations = Declarations::builder()
        .standard_functions()
        .ident("x", Int)
        .ident("s", String)
        .function("f", vec![Overload::new(Bool, vec![Int])])
        .function("f", vec![Overload::new(Bool, vec![String])])
        .build();
    check("f(x)", &declarations).expect("int overload resolves");
    check("f(s)", &declarations).expect("string overload resolves");
}

#[test]
fn accepts_has_operator_overloads() {
    let cases: Vec<(&str, Declarations)> = vec![
        // string : string — a quoted value and a bare-identifier value.
        (r#"name:"acme""#, decls(|b| b.ident("name", String))),
        ("name:acme", decls(|b| b.ident("name", String))),
        // map<string,string> : string — presence of a key.
        (
            "labels:production",
            decls(|b| b.ident("labels", map(String, String))),
        ),
        // list<string> : string — presence of an element.
        ("tags:urgent", decls(|b| b.ident("tags", list(String)))),
        // timestamp : "*" — the presence-only wildcard.
        (
            "create_time:*",
            decls(|b| b.ident("create_time", Timestamp)),
        ),
    ];
    for (filter, declarations) in cases {
        check(filter, &declarations)
            .unwrap_or_else(|e| panic!("filter {filter:?} should check: {e}"));
    }
}

#[test]
fn rejects_invalid_has_operands() {
    // An operand whose type has no `:` overload resolves to no matching overload.
    let no_overload: Vec<(&str, Declarations)> = vec![
        ("count:foo", decls(|b| b.ident("count", Int))),
        ("flag:foo", decls(|b| b.ident("flag", Bool))),
    ];
    for (filter, declarations) in no_overload {
        match check(filter, &declarations) {
            Err(Error::Type(message)) => assert!(
                message.contains("no matching overload"),
                "filter {filter:?}: message {message:?} lacks 'no matching overload'"
            ),
            other => panic!("filter {filter:?}: expected a type error, got {other:?}"),
        }
    }
    // A timestamp field can only be presence-checked with the `*` wildcard, not
    // membership-tested against a concrete value.
    let declarations = decls(|b| b.ident("create_time", Timestamp));
    match check(r#"create_time:"2022-08-12T22:22:22+01:00""#, &declarations) {
        Err(Error::Type(message)) => assert!(
            message.contains("only supports the wildcard"),
            "message {message:?} lacks the timestamp wildcard restriction"
        ),
        other => panic!("expected a wildcard-restriction type error, got {other:?}"),
    }
}
