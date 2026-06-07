//! Table tests for the public [`parse`] over the full AIP-160 grammar: the
//! parser builds the native [`Expr`] AST for comparisons, the logical operators
//! (`AND` / `OR` / `NOT`, plus the implicit `FUZZY` AND), functions, and
//! parenthesised composites.
//!
//! Ported from `aip-go`'s `parser_test.go`, excluding the `:` (has) cases (which
//! land with the has-operator slice) and its invalid-UTF-8 case (a Rust `&str`
//! is always valid UTF-8).

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

/// Left-associate `args` under `function` — how the parser nests the repeated
/// logical operators (`a b c` -> `FUZZY(FUZZY(a, b), c)`).
fn fold(function: &str, args: Vec<Expr>) -> Expr {
    let mut iter = args.into_iter();
    let mut acc = iter.next().expect("at least one arg");
    for arg in iter {
        acc = call(function, vec![acc, arg]);
    }
    acc
}

fn seq(args: Vec<Expr>) -> Expr {
    fold("FUZZY", args)
}

fn and(args: Vec<Expr>) -> Expr {
    fold("AND", args)
}

fn or(args: Vec<Expr>) -> Expr {
    fold("OR", args)
}

fn not(arg: Expr) -> Expr {
    call("NOT", vec![arg])
}

#[test]
fn parses_logical_composition() {
    let cases = [
        // Implicit AND (FUZZY) between space-separated factors.
        (
            "New York Giants",
            seq(vec![ident("New"), ident("York"), ident("Giants")]),
        ),
        (
            "New York Giants OR Yankees",
            seq(vec![
                ident("New"),
                ident("York"),
                or(vec![ident("Giants"), ident("Yankees")]),
            ]),
        ),
        // Parentheses group an OR without changing the sequence's shape.
        (
            "New York (Giants OR Yankees)",
            seq(vec![
                ident("New"),
                ident("York"),
                or(vec![ident("Giants"), ident("Yankees")]),
            ]),
        ),
        // Explicit AND binds looser than the implicit sequence AND.
        (
            "a b AND c AND d",
            and(vec![
                seq(vec![ident("a"), ident("b")]),
                ident("c"),
                ident("d"),
            ]),
        ),
        (
            "(a b) AND c AND d",
            and(vec![
                seq(vec![ident("a"), ident("b")]),
                ident("c"),
                ident("d"),
            ]),
        ),
        (
            "a < 10 OR a >= 100",
            or(vec![
                call("<", vec![ident("a"), int(10)]),
                call(">=", vec![ident("a"), int(100)]),
            ]),
        ),
        ("a OR b OR c", or(vec![ident("a"), ident("b"), ident("c")])),
        ("NOT (a OR b)", not(or(vec![ident("a"), ident("b")]))),
    ];
    for (filter, expected) in cases {
        assert_eq!(parse(filter).expect(filter), expected, "filter: {filter}");
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
        // A parenthesised negative literal is still just that literal.
        (
            "foo >= (-2.4)",
            call(">=", vec![ident("foo"), double(-2.4)]),
        ),
        // A leading `-` on a non-literal restriction negates the whole thing.
        (
            "-2.5 >= -2.4",
            not(call(">=", vec![double(2.5), double(-2.4)])),
        ),
        ("a < 10", call("<", vec![ident("a"), int(10)])),
        ("a <= 10", call("<=", vec![ident("a"), int(10)])),
        (
            "package = com.google",
            call("=", vec![ident("package"), select(ident("com"), "google")]),
        ),
        (
            "package=com.google",
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
fn parses_functions() {
    let cases = [
        ("time.now()", call("time.now", vec![])),
        (
            "regex(m.key, '^.*prod.*$')",
            call(
                "regex",
                vec![select(ident("m"), "key"), string("^.*prod.*$")],
            ),
        ),
        ("math.mem('30mb')", call("math.mem", vec![string("30mb")])),
        (
            "experiment.rollout <= cohort(request.user)",
            call(
                "<=",
                vec![
                    select(ident("experiment"), "rollout"),
                    call("cohort", vec![select(ident("request"), "user")]),
                ],
            ),
        ),
        (
            "(msg.endsWith('world') AND retries < 10)",
            and(vec![
                call("msg.endsWith", vec![string("world")]),
                call("<", vec![ident("retries"), int(10)]),
            ]),
        ),
        (
            "(endsWith(msg, 'world') AND retries < 10)",
            and(vec![
                call("endsWith", vec![ident("msg"), string("world")]),
                call("<", vec![ident("retries"), int(10)]),
            ]),
        ),
        // `timestamp(...)` / `duration(...)` parse as ordinary function calls;
        // their overloads land with the timestamp/duration slice.
        (
            r#"timestamp("2012-04-21T11:30:00-04:00")"#,
            call("timestamp", vec![string("2012-04-21T11:30:00-04:00")]),
        ),
        ("duration('32s')", call("duration", vec![string("32s")])),
    ];
    for (filter, expected) in cases {
        assert_eq!(parse(filter).expect(filter), expected, "filter: {filter}");
    }
}

#[test]
fn parses_multiline_filter() {
    let filter = r#"
        start_time > timestamp("2006-01-02T15:04:05+07:00") AND
        (driver = "driver1" OR start_driver = "driver1" OR end_driver = "driver1")
    "#;
    let expected = and(vec![
        call(
            ">",
            vec![
                ident("start_time"),
                call("timestamp", vec![string("2006-01-02T15:04:05+07:00")]),
            ],
        ),
        or(vec![
            call("=", vec![ident("driver"), string("driver1")]),
            call("=", vec![ident("start_driver"), string("driver1")]),
            call("=", vec![ident("end_driver"), string("driver1")]),
        ]),
    ]);
    assert_eq!(parse(filter).expect("multiline filter parses"), expected);
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
        (r#"x = "\303\277""#, "Ã¿"),
        (r#"x = "☺☺""#, "☺☺"),
        (r#"x = "[ 'hello' ]""#, "[ 'hello' ]"),
        (r#"x = "[ \"hello\" ]""#, "[ \"hello\" ]"),
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
        // A parenthesised composite cannot be a comparison operand.
        ("(-2.5) >= -2.4", "unexpected token", 7),
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
