//! Table tests for the public [`check`]: `field OP literal` is type-checked
//! against a [`Declarations`] allowlist, rejecting undeclared identifiers and
//! type mismatches and requiring a boolean result.

use aip_filtering::{check, Declarations, Error, Type};

/// Build declarations from `(name, type)` pairs.
fn decls(idents: &[(&str, Type)]) -> Declarations {
    let mut builder = Declarations::builder();
    for (name, ty) in idents {
        builder = builder.ident(name, ty.clone());
    }
    builder.build().expect("declarations build")
}

#[test]
fn accepts_well_typed_comparisons() {
    let cases: &[(&str, &[(&str, Type)])] = &[
        // A bare bool identifier is a valid (bool-typed) filter.
        ("prod", &[("prod", Type::Bool)]),
        (r#"msg != 'hello'"#, &[("msg", Type::String)]),
        ("1 > 0", &[]),
        ("2.5 >= 2.4", &[]),
        ("foo >= -2.4", &[("foo", Type::Double)]),
        // A double field compared to an int literal (the float_int overload).
        ("foo = 3", &[("foo", Type::Double)]),
        ("foo <= 3", &[("foo", Type::Double)]),
        ("a < 10", &[("a", Type::Int)]),
        (
            r#"display_name = "Acme""#,
            &[("display_name", Type::String)],
        ),
        // A dotted member resolves against its declared full name.
        (r#"book.author = "Boye""#, &[("book.author", Type::String)]),
    ];
    for (filter, idents) in cases {
        let declarations = decls(idents);
        check(filter, &declarations)
            .unwrap_or_else(|e| panic!("filter {filter:?} should check: {e}"));
    }
}

#[test]
fn rejects_undeclared_identifiers() {
    let declarations = decls(&[]);
    match check(r#"name = "x""#, &declarations) {
        Err(Error::UndeclaredIdent(name)) => assert_eq!(name, "name"),
        other => panic!("expected an undeclared-identifier error, got {other:?}"),
    }
    // A dotted member is reported by its full path.
    match check(r#"book.author = "x""#, &declarations) {
        Err(Error::UndeclaredIdent(name)) => assert_eq!(name, "book.author"),
        other => panic!("expected an undeclared-identifier error, got {other:?}"),
    }
}

#[test]
fn rejects_type_mismatches() {
    let cases: &[(&str, &[(&str, Type)])] = &[
        // String field compared to an int literal: no matching overload.
        ("count = 3", &[("count", Type::String)]),
        // Int field ordered against a string literal.
        (r#"age > "x""#, &[("age", Type::Int)]),
        // The int/double overload is one-directional (no int-then-double).
        ("count = 3.0", &[("count", Type::Int)]),
    ];
    for (filter, idents) in cases {
        let declarations = decls(idents);
        match check(filter, &declarations) {
            Err(Error::Type(_)) => {}
            other => panic!("filter {filter:?}: expected a type error, got {other:?}"),
        }
    }
}

#[test]
fn rejects_non_bool_results() {
    for (filter, idents) in [("-30", &[][..]), ("name", &[("name", Type::String)][..])] {
        let declarations = decls(idents);
        match check(filter, &declarations) {
            Err(Error::Type(_)) => {}
            other => panic!("filter {filter:?}: expected a non-bool type error, got {other:?}"),
        }
    }
}
