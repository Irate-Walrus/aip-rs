//! Table tests for the public [`parse`] over the comparison slice: the parser
//! builds the native [`Expr`] AST for `field OP literal` restrictions.

use aip_filtering::{parse, Constant, Error, Expr};

fn ident(name: &str) -> Expr {
    Expr::Ident(name.to_string())
}

fn int(value: i64) -> Expr {
    Expr::Const(Constant::Int(value))
}

fn double(value: f64) -> Expr {
    Expr::Const(Constant::Double(value))
}

fn string(value: &str) -> Expr {
    Expr::Const(Constant::String(value.to_string()))
}

fn select(operand: Expr, field: &str) -> Expr {
    Expr::Select {
        operand: Box::new(operand),
        field: field.to_string(),
    }
}

fn call(function: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        function: function.to_string(),
        args,
    }
}

#[test]
fn parses_comparisons_and_members() {
    let cases = [
        // A bare comparable is a restriction with no comparator.
        ("prod", ident("prod")),
        ("-30", int(-30)),
        ("0x2A", int(42)),
        // Each comparison operator over numeric, string, and member operands.
        ("1 > 0", call(">", vec![int(1), int(0)])),
        ("2.5 >= 2.4", call(">=", vec![double(2.5), double(2.4)])),
        ("foo >= -2.4", call(">=", vec![ident("foo"), double(-2.4)])),
        ("a < 10", call("<", vec![ident("a"), int(10)])),
        ("a <= 10", call("<=", vec![ident("a"), int(10)])),
        (
            "package = com.google",
            call("=", vec![ident("package"), select(ident("com"), "google")]),
        ),
        (
            r#"msg != 'hello'"#,
            call("!=", vec![ident("msg"), string("hello")]),
        ),
        (
            r#"msg != "hello""#,
            call("!=", vec![ident("msg"), string("hello")]),
        ),
        (
            r#"annotations.schedule = "test""#,
            call(
                "=",
                vec![select(ident("annotations"), "schedule"), string("test")],
            ),
        ),
        (
            "yesterday < request.time",
            call(
                "<",
                vec![ident("yesterday"), select(ident("request"), "time")],
            ),
        ),
        // A dotted member, including a numeric field segment.
        (
            "expr.type_map.1.type",
            select(select(select(ident("expr"), "type_map"), "1"), "type"),
        ),
    ];
    for (filter, expected) in cases {
        assert_eq!(parse(filter).expect(filter), expected, "filter: {filter}");
    }
}

#[test]
fn parses_string_escapes() {
    let cases = [
        (r#"x = """#, ""),
        (r#"x = "a\"b""#, "a\"b"),
        (r#"x = "\\""#, "\\"),
        (r#"x = "\n\t""#, "\n\t"),
        (r#"x = "☺""#, "☺"),
        (r#"x = "\377""#, "ÿ"),
    ];
    for (filter, want) in cases {
        let expected = call("=", vec![ident("x"), string(want)]);
        assert_eq!(parse(filter).expect(filter), expected, "filter: {filter}");
    }
}

#[test]
fn rejects_syntax_errors_with_a_position() {
    // (filter, message-substring, expected byte offset)
    let cases = [
        ("", "empty", 0),
        ("<", "unexpected token", 0),
        (r#"a = "foo"#, "unterminated", 4),
        ("a b", "trailing", 2),
        // `AND` is a later slice, so it surfaces as a trailing token here.
        ("a AND b", "trailing", 2),
        ("a = ", "end of input", 4),
    ];
    for (filter, needle, offset) in cases {
        match parse(filter) {
            Err(Error::Syntax { message, position }) => {
                assert!(
                    message.contains(needle),
                    "filter {filter:?}: message {message:?} lacks {needle:?}"
                );
                assert_eq!(position, offset, "filter {filter:?}: wrong position");
            }
            other => panic!("filter {filter:?}: expected a syntax error, got {other:?}"),
        }
    }
}
