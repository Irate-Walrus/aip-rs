//! Tests for [`apply_macros`]: a macro inspects each node and may rewrite it,
//! after which the filter is re-type-checked against the supplied declarations.
//!
//! Ported from `aip-go`'s `macro_test.go`. `aip-go`'s experimental
//! `AddDeclarations` / `ReplaceWithDeclarations` cursor methods are not ported —
//! `ApplyMacros` ignores the declaration options they collect, so those cases
//! are equivalent to a plain `replace` re-checked against the given
//! declarations, which is what the corresponding tests here exercise.

use aip_filtering::Type::{Int, String};
use aip_filtering::{
    apply_macros, check, Constant, Cursor, Declarations, DeclarationsBuilder, Error, Expr,
};

fn decls(build: impl FnOnce(DeclarationsBuilder) -> DeclarationsBuilder) -> Declarations {
    build(Declarations::builder().standard_functions())
        .build()
        .expect("declarations build")
}

fn ident(name: &str) -> Expr {
    Expr::Ident(name.to_string())
}

fn int(value: i64) -> Expr {
    Expr::Const(Constant::Int(value))
}

fn string(value: &str) -> Expr {
    Expr::Const(Constant::String(value.to_string()))
}

fn call(function: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        function: function.to_string(),
        args,
    }
}

fn eq(lhs: Expr, rhs: Expr) -> Expr {
    call("=", vec![lhs, rhs])
}

fn and(lhs: Expr, rhs: Expr) -> Expr {
    call("AND", vec![lhs, rhs])
}

fn not(arg: Expr) -> Expr {
    call("NOT", vec![arg])
}

/// Replace `Ident(from)` with `Ident(to)` wherever it appears.
fn rename<'a>(from: &'a str, to: &'a str) -> impl Fn(&mut Cursor) + 'a {
    move |cursor| {
        let matched = matches!(cursor.expr(), Expr::Ident(name) if name == from);
        if matched {
            cursor.replace(ident(to));
        }
    }
}

#[test]
fn rewrites_not_equals_to_negated_equals() {
    let filter =
        check(r#"name != "test""#, &decls(|b| b.ident("name", String))).expect("filter checks");
    let to_negated: &dyn Fn(&mut Cursor) = &|cursor| {
        let new = match cursor.expr() {
            Expr::Call { function, args } if function == "!=" && args.len() == 2 => {
                Some(not(eq(args[0].clone(), args[1].clone())))
            }
            _ => None,
        };
        if let Some(new) = new {
            cursor.replace(new);
        }
    };
    let result = apply_macros(filter, &decls(|b| b.ident("name", String)), &[to_negated])
        .expect("macro applies");
    assert_eq!(result.expr, not(eq(ident("name"), string("test"))));
}

#[test]
fn noop_when_macro_does_not_match() {
    let filter = check(r#"name = "test""#, &decls(|b| b.ident("name", String))).expect("checks");
    let other = rename("other_name", "renamed");
    let other: &dyn Fn(&mut Cursor) = &other;
    let result =
        apply_macros(filter, &decls(|b| b.ident("name", String)), &[other]).expect("macro applies");
    assert_eq!(result.expr, eq(ident("name"), string("test")));
}

#[test]
fn empty_macro_list_is_noop() {
    let filter = check(r#"name = "test""#, &decls(|b| b.ident("name", String))).expect("checks");
    let result =
        apply_macros(filter, &decls(|b| b.ident("name", String)), &[]).expect("macro applies");
    assert_eq!(result.expr, eq(ident("name"), string("test")));
}

#[test]
fn applies_multiple_macros_in_sequence() {
    let filter = check(
        "x = 5 AND y = 10",
        &decls(|b| b.ident("x", Int).ident("y", Int)),
    )
    .expect("checks");
    let rename_x = rename("x", "x_renamed");
    let rename_y = rename("y", "y_renamed");
    let macros: &[&dyn Fn(&mut Cursor)] = &[&rename_x, &rename_y];
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("x_renamed", Int).ident("y_renamed", Int)),
        macros,
    )
    .expect("macros apply");
    assert_eq!(
        result.expr,
        and(
            eq(ident("x_renamed"), int(5)),
            eq(ident("y_renamed"), int(10))
        )
    );
}

#[test]
fn rewrites_inside_nested_composite() {
    let filter = check(
        "(x = 5) AND (y = 10)",
        &decls(|b| b.ident("x", Int).ident("y", Int)),
    )
    .expect("checks");
    // Rewrite `x = <n>` to `x_renamed = <n>`, leaving `y = 10` untouched.
    let rewrite_x: &dyn Fn(&mut Cursor) = &|cursor| {
        let new = match cursor.expr() {
            Expr::Call { function, args }
                if function == "=" && matches!(&args[0], Expr::Ident(n) if n == "x") =>
            {
                Some(eq(ident("x_renamed"), args[1].clone()))
            }
            _ => None,
        };
        if let Some(new) = new {
            cursor.replace(new);
        }
    };
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("x_renamed", Int).ident("y", Int)),
        &[rewrite_x],
    )
    .expect("macro applies");
    assert_eq!(
        result.expr,
        and(eq(ident("x_renamed"), int(5)), eq(ident("y"), int(10)))
    );
}

#[test]
fn rewrites_repeated_identifier() {
    let filter = check(
        r#"name = "test" AND name = "test2""#,
        &decls(|b| b.ident("name", String)),
    )
    .expect("checks");
    let rename_name = rename("name", "name_renamed");
    let rename_name: &dyn Fn(&mut Cursor) = &rename_name;
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("name_renamed", String)),
        &[rename_name],
    )
    .expect("macro applies");
    assert_eq!(
        result.expr,
        and(
            eq(ident("name_renamed"), string("test")),
            eq(ident("name_renamed"), string("test2")),
        )
    );
}

#[test]
fn rewrites_less_than_to_less_equals() {
    let filter = check("age < 18", &decls(|b| b.ident("age", Int))).expect("checks");
    let tighten: &dyn Fn(&mut Cursor) = &|cursor| {
        let new = match cursor.expr() {
            Expr::Call { function, args } if function == "<" => match &args[1] {
                Expr::Const(Constant::Int(v)) => {
                    Some(call("<=", vec![args[0].clone(), int(v - 1)]))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(new) = new {
            cursor.replace(new);
        }
    };
    let result =
        apply_macros(filter, &decls(|b| b.ident("age", Int)), &[tighten]).expect("macro applies");
    assert_eq!(result.expr, call("<=", vec![ident("age"), int(17)]));
}

#[test]
fn rewrites_or_to_de_morgan() {
    let filter = check(
        "x = 1 OR y = 2",
        &decls(|b| b.ident("x", Int).ident("y", Int)),
    )
    .expect("checks");
    let de_morgan: &dyn Fn(&mut Cursor) = &|cursor| {
        let new = match cursor.expr() {
            Expr::Call { function, args } if function == "OR" && args.len() == 2 => {
                Some(not(and(not(args[0].clone()), not(args[1].clone()))))
            }
            _ => None,
        };
        if let Some(new) = new {
            cursor.replace(new);
        }
    };
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("x", Int).ident("y", Int)),
        &[de_morgan],
    )
    .expect("macro applies");
    assert_eq!(
        result.expr,
        not(and(
            not(eq(ident("x"), int(1))),
            not(eq(ident("y"), int(2)))
        ))
    );
}

#[test]
fn rewrites_select_to_ident() {
    let filter = check(
        r#"user.name = "John""#,
        &decls(|b| {
            b.ident(
                "user",
                aip_filtering::Type::Map(Box::new(String), Box::new(String)),
            )
        }),
    )
    .expect("checks");
    let flatten: &dyn Fn(&mut Cursor) = &|cursor| {
        let matched = matches!(
            cursor.expr(),
            Expr::Select { operand, field }
                if field == "name" && matches!(operand.as_ref(), Expr::Ident(n) if n == "user")
        );
        if matched {
            cursor.replace(ident("user_name"));
        }
    };
    let result = apply_macros(filter, &decls(|b| b.ident("user_name", String)), &[flatten])
        .expect("macro applies");
    assert_eq!(result.expr, eq(ident("user_name"), string("John")));
}

#[test]
fn stops_traversal_after_replacement() {
    let filter = check(
        "x = 5 AND y = 10",
        &decls(|b| b.ident("x", Int).ident("y", Int)),
    )
    .expect("checks");
    // The first macro replaces the whole AND, so the second never runs.
    let replace_and: &dyn Fn(&mut Cursor) = &|cursor| {
        let matched = matches!(cursor.expr(), Expr::Call { function, .. } if function == "AND");
        if matched {
            cursor.replace(eq(ident("combined"), int(15)));
        }
    };
    let poison: &dyn Fn(&mut Cursor) = &|cursor| {
        if matches!(cursor.expr(), Expr::Ident(_)) {
            cursor.replace(ident("should_not_appear"));
        }
    };
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("combined", Int)),
        &[replace_and, poison],
    )
    .expect("macro applies");
    assert_eq!(result.expr, eq(ident("combined"), int(15)));
}

#[test]
fn adds_to_all_int_constants() {
    let filter = check(
        "(a = 1 AND b = 2) OR (c = 3 AND d = 4)",
        &decls(|b| {
            b.ident("a", Int)
                .ident("b", Int)
                .ident("c", Int)
                .ident("d", Int)
        }),
    )
    .expect("checks");
    let add_ten: &dyn Fn(&mut Cursor) = &|cursor| {
        let new = match cursor.expr() {
            Expr::Const(Constant::Int(v)) => Some(int(v + 10)),
            _ => None,
        };
        if let Some(new) = new {
            cursor.replace(new);
        }
    };
    let result = apply_macros(
        filter,
        &decls(|b| {
            b.ident("a", Int)
                .ident("b", Int)
                .ident("c", Int)
                .ident("d", Int)
        }),
        &[add_ten],
    )
    .expect("macro applies");
    let expected = call(
        "OR",
        vec![
            and(eq(ident("a"), int(11)), eq(ident("b"), int(12))),
            and(eq(ident("c"), int(13)), eq(ident("d"), int(14))),
        ],
    );
    assert_eq!(result.expr, expected);
}

#[test]
fn rejects_ill_typed_rewrite() {
    // The rewrite is type-checked against the new declarations, where `name` is
    // an int — so `name = "test"` no longer resolves an overload.
    let filter = check(r#"name = "test""#, &decls(|b| b.ident("name", String))).expect("checks");
    let touch_name = rename("name", "name");
    let touch_name: &dyn Fn(&mut Cursor) = &touch_name;
    match apply_macros(filter, &decls(|b| b.ident("name", Int)), &[touch_name]) {
        Err(Error::Type(message)) => assert!(
            message.contains("no matching overload"),
            "message {message:?} lacks 'no matching overload'"
        ),
        other => panic!("expected a type error, got {other:?}"),
    }
}

#[test]
fn rejects_rewrite_into_undeclared_identifier() {
    let filter = check(r#"name = "test""#, &decls(|b| b.ident("name", String))).expect("checks");
    let rename_name = rename("name", "renamed_name");
    let rename_name: &dyn Fn(&mut Cursor) = &rename_name;
    // The new declarations don't declare `renamed_name`.
    match apply_macros(filter, &decls(|b| b.ident("name", String)), &[rename_name]) {
        Err(Error::UndeclaredIdent(name)) => assert_eq!(name, "renamed_name"),
        other => panic!("expected an undeclared-identifier error, got {other:?}"),
    }
}

#[test]
fn rewrites_with_declaration_supplied_in_new_declarations() {
    let filter = check(r#"name = "test""#, &decls(|b| b.ident("name", String))).expect("checks");
    let rename_name = rename("name", "renamed_name");
    let rename_name: &dyn Fn(&mut Cursor) = &rename_name;
    let result = apply_macros(
        filter,
        &decls(|b| b.ident("renamed_name", String)),
        &[rename_name],
    )
    .expect("macro applies");
    assert_eq!(result.expr, eq(ident("renamed_name"), string("test")));
}
