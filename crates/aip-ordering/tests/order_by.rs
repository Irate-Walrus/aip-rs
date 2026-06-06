//! Ported from `go.einride.tech/aip/ordering`'s `orderby_test.go` and
//! `request_test.go`.

use aip_ordering::{parse_order_by, Error, OrderBy, OrderByField, OrderByRequest};

fn field(path: &str, desc: bool) -> OrderByField {
    OrderByField {
        path: path.to_owned(),
        desc,
    }
}

// ---- OrderBy::from_str ----

#[test]
fn parse_empty_is_ok() {
    let got: OrderBy = "".parse().expect("empty string is valid");
    assert_eq!(got, OrderBy::default());
}

#[test]
fn parse_multi_field_desc_asc() {
    let got: OrderBy = "foo desc, bar".parse().unwrap();
    assert_eq!(
        got,
        OrderBy {
            fields: vec![field("foo", true), field("bar", false)],
        }
    );
}

#[test]
fn parse_nested_path() {
    let got: OrderBy = "foo.bar".parse().unwrap();
    assert_eq!(
        got,
        OrderBy {
            fields: vec![field("foo.bar", false)],
        }
    );
}

#[test]
fn parse_whitespace_trimmed() {
    let got: OrderBy = " foo , bar desc ".parse().unwrap();
    assert_eq!(
        got,
        OrderBy {
            fields: vec![field("foo", false), field("bar", true)],
        }
    );
}

#[test]
fn parse_trailing_comma_is_error() {
    let err = "foo,".parse::<OrderBy>().unwrap_err();
    assert!(
        matches!(err, Error::Syntax(_)),
        "expected Syntax error, got {err:?}"
    );
}

#[test]
fn parse_leading_comma_is_error() {
    let err = ",".parse::<OrderBy>().unwrap_err();
    assert!(
        matches!(err, Error::Syntax(_)),
        "expected Syntax error, got {err:?}"
    );
}

#[test]
fn parse_comma_before_field_is_error() {
    let err = ",foo".parse::<OrderBy>().unwrap_err();
    assert!(
        matches!(err, Error::Syntax(_)),
        "expected Syntax error, got {err:?}"
    );
}

#[test]
fn parse_slash_is_invalid_character() {
    let err = "foo/bar".parse::<OrderBy>().unwrap_err();
    match &err {
        Error::Syntax(msg) => assert!(msg.contains('/'), "expected '/' in message, got: {msg}"),
        other => panic!("expected Syntax error, got {other:?}"),
    }
}

#[test]
fn parse_extra_word_is_error() {
    // "foo bar" has two words but neither is "asc"/"desc"
    let err = "foo bar".parse::<OrderBy>().unwrap_err();
    assert!(
        matches!(err, Error::Syntax(_)),
        "expected Syntax error, got {err:?}"
    );
}

// ---- OrderBy::validate_for_paths ----

#[test]
fn validate_empty_order_by_against_empty_paths() {
    OrderBy::default()
        .validate_for_paths(&[])
        .expect("empty order_by is valid against any allow-list");
}

#[test]
fn validate_known_paths() {
    let ob = OrderBy {
        fields: vec![field("name", false), field("author", false)],
    };
    ob.validate_for_paths(&["name", "author", "read"])
        .expect("all fields are in the allow-list");
}

#[test]
fn validate_unknown_path_is_error() {
    let ob = OrderBy {
        fields: vec![field("name", false), field("foo", false)],
    };
    let err = ob
        .validate_for_paths(&["name", "author", "read"])
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "foo"),
        other => panic!("expected UnknownField for `foo`, got {other:?}"),
    }
}

#[test]
fn validate_known_nested_path() {
    let ob = OrderBy {
        fields: vec![field("name", false), field("book.name", false)],
    };
    ob.validate_for_paths(&["name", "book.name", "book.author", "book.read"])
        .expect("nested path is in the allow-list");
}

#[test]
fn validate_unknown_nested_path_is_error() {
    let ob = OrderBy {
        fields: vec![field("name", false), field("book.foo", false)],
    };
    let err = ob
        .validate_for_paths(&["name", "book.name", "book.author", "book.read"])
        .unwrap_err();
    match err {
        Error::UnknownField(path) => assert_eq!(path, "book.foo"),
        other => panic!("expected UnknownField for `book.foo`, got {other:?}"),
    }
}

// ---- OrderByField::sub_fields ----

#[test]
fn sub_fields_empty_path_is_empty() {
    let f = field("", false);
    let got: Vec<&str> = f.sub_fields().collect();
    assert_eq!(got, Vec::<&str>::new());
}

#[test]
fn sub_fields_single_segment() {
    let f = field("foo", false);
    let got: Vec<&str> = f.sub_fields().collect();
    assert_eq!(got, vec!["foo"]);
}

#[test]
fn sub_fields_multi_segment() {
    let f = field("foo.bar", false);
    let got: Vec<&str> = f.sub_fields().collect();
    assert_eq!(got, vec!["foo", "bar"]);
}

// ---- parse_order_by ----

struct MockRequest {
    order_by: String,
}

impl OrderByRequest for MockRequest {
    fn order_by(&self) -> &str {
        &self.order_by
    }
}

#[test]
fn parse_order_by_success() {
    let req = MockRequest {
        order_by: "foo asc,bar desc".to_owned(),
    };
    let got = parse_order_by(&req).unwrap();
    assert_eq!(
        got,
        OrderBy {
            fields: vec![field("foo", false), field("bar", true)],
        }
    );
}

#[test]
fn parse_order_by_invalid_character() {
    let req = MockRequest {
        order_by: "/foo".to_owned(),
    };
    let err = parse_order_by(&req).unwrap_err();
    match &err {
        Error::Syntax(msg) => assert!(msg.contains('/'), "expected '/' in message, got: {msg}"),
        other => panic!("expected Syntax error, got {other:?}"),
    }
}
