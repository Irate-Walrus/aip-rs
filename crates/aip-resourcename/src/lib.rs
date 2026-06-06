//! AIP-122 resource names: parse, format, match, and validate.
//!
//! This crate is pure string work — it has no protobuf dependency.
//!
//! Unlike `aip-go`'s variadic `Sscan`/`Sprint`, parsing a name against a
//! [`Pattern`] yields named [`Captures`] (a `regex::Captures`-style API).
//! See `docs/adr/0002-idiomatic-resourcename-api.md`.
//!
//! See <https://google.aip.dev/122>.

use std::collections::{BTreeMap, BTreeSet};

/// Errors produced when parsing, validating, or matching resource names.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("empty resource name")]
    Empty,
    #[error("segment {index} is empty")]
    EmptySegment { index: usize },
    #[error("segment {segment:?} is not a valid DNS name")]
    InvalidDnsName { segment: String },
    #[error("resource name does not match pattern {pattern:?}")]
    PatternMismatch { pattern: String },
    #[error("invalid pattern: {0}")]
    InvalidPattern(String),
    #[error("missing value for variable {name:?}")]
    MissingVariable { name: String },
    #[error("variable {name:?} is not declared in the pattern")]
    UnknownVariable { name: String },
}

/// The wildcard segment, `-` (matches any single resource ID).
pub const WILDCARD: &str = "-";
/// The revision separator, `@` (as in `books/les-miserables@1.0.0`).
pub const REVISION_SEPARATOR: char = '@';

/// A parsed resource-name pattern, e.g. `shippers/{shipper}/sites/{site}`.
#[derive(Debug, Clone)]
pub struct Pattern {
    segments: Vec<PatternSegment>,
}

/// One `/`-separated component of a parsed [`Pattern`].
#[derive(Debug, Clone)]
enum PatternSegment {
    /// A fixed segment (a collection ID or resource ID).
    Literal(String),
    /// A `{name}` placeholder; carries the variable name.
    Variable(String),
}

impl Pattern {
    /// Parse a pattern string into a reusable matcher.
    ///
    /// A pattern is a `/`-separated sequence of [`Literal`] segments and
    /// `{variable}` segments. A single leading `/` is tolerated and ignored.
    /// Full resource names (a `//service/...` prefix), [`Wildcard`](WILDCARD)
    /// (`-`) segments, empty segments, malformed `{…}` variables, and repeated
    /// variable names are rejected with [`Error::InvalidPattern`].
    pub fn parse(pattern: &str) -> Result<Self, Error> {
        // A full resource name carries a service name and is not a pattern.
        if pattern.starts_with("//") {
            return Err(Error::InvalidPattern(
                "full resource name is not a valid pattern".to_string(),
            ));
        }
        // Tolerate (and drop) a single leading slash, matching aip-go's scanner.
        let body = pattern.strip_prefix('/').unwrap_or(pattern);
        if body.is_empty() {
            return Err(Error::InvalidPattern("pattern is empty".to_string()));
        }
        let mut segments = Vec::new();
        let mut seen = BTreeSet::new();
        for raw in body.split('/') {
            if raw.is_empty() {
                return Err(Error::InvalidPattern(
                    "pattern has an empty segment".to_string(),
                ));
            }
            if raw == WILDCARD {
                return Err(Error::InvalidPattern(
                    "wildcard `-` is not allowed in a pattern".to_string(),
                ));
            }
            match variable_name(raw) {
                Some("") => {
                    return Err(Error::InvalidPattern(
                        "variable segment has an empty name".to_string(),
                    ));
                }
                Some(name) => {
                    if !seen.insert(name) {
                        return Err(Error::InvalidPattern(format!(
                            "variable {name:?} appears more than once"
                        )));
                    }
                    segments.push(PatternSegment::Variable(name.to_string()));
                }
                None if raw.contains('{') || raw.contains('}') => {
                    return Err(Error::InvalidPattern(format!(
                        "segment {raw:?} is a malformed variable"
                    )));
                }
                None => segments.push(PatternSegment::Literal(raw.to_string())),
            }
        }
        Ok(Self { segments })
    }

    /// Match a concrete resource name, binding each variable by name.
    ///
    /// Returns named [`Captures`] when `name` matches, or `None` otherwise. The
    /// name may be a full resource name; the `//service` prefix is skipped
    /// before matching. A name that is shorter or
    /// longer than the pattern, or whose literal segments differ, does not
    /// match; a variable binds to any non-empty segment in its position.
    pub fn match_name<'a>(&'a self, name: &'a str) -> Option<Captures<'a>> {
        let segments = name_segments(name);
        if segments.len() != self.segments.len() {
            return None;
        }
        let mut vars = BTreeMap::new();
        for (pattern, segment) in self.segments.iter().zip(segments) {
            // A concrete resource name must not contain `{variable}` segments.
            if variable_name(segment).is_some() {
                return None;
            }
            match pattern {
                PatternSegment::Literal(literal) => {
                    if literal != segment {
                        return None;
                    }
                }
                PatternSegment::Variable(name) => {
                    if segment.is_empty() {
                        return None;
                    }
                    vars.insert(name.as_str(), segment);
                }
            }
        }
        Some(Captures { vars })
    }

    /// Format a resource name from `(variable, value)` pairs.
    ///
    /// Each `{variable}` in the pattern is replaced by its supplied value;
    /// literal segments are emitted verbatim. Returns
    /// [`Error::MissingVariable`] if a pattern variable has no supplied value
    /// and [`Error::UnknownVariable`] if a supplied variable is not declared in
    /// the pattern. An empty value is permitted (it is present, just empty).
    pub fn format<'a, I>(&self, vars: I) -> Result<String, Error>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let provided: BTreeMap<&str, &str> = vars.into_iter().collect();
        for &name in provided.keys() {
            if !self.declares(name) {
                return Err(Error::UnknownVariable {
                    name: name.to_string(),
                });
            }
        }
        let mut out = String::new();
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                out.push('/');
            }
            match segment {
                PatternSegment::Literal(literal) => out.push_str(literal),
                PatternSegment::Variable(name) => {
                    let value = provided
                        .get(name.as_str())
                        .ok_or_else(|| Error::MissingVariable { name: name.clone() })?;
                    out.push_str(value);
                }
            }
        }
        Ok(out)
    }

    /// Reports whether the pattern declares a variable named `name`.
    fn declares(&self, name: &str) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, PatternSegment::Variable(v) if v == name))
    }
}

/// Variables bound by [`Pattern::match_name`].
#[derive(Debug, Clone, Default)]
pub struct Captures<'a> {
    vars: BTreeMap<&'a str, &'a str>,
}

impl<'a> Captures<'a> {
    /// Get a bound variable's value by name.
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.vars.get(name).copied()
    }

    /// Iterate over `(variable, value)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a str)> + '_ {
        self.vars.iter().map(|(k, v)| (*k, *v))
    }
}

/// Reports whether `name` matches `pattern`.
///
/// Mirrors [`Pattern::match_name`]; a `pattern` that fails to parse never
/// matches.
pub fn is_match(pattern: &str, name: &str) -> bool {
    Pattern::parse(pattern)
        .map(|p| p.match_name(name).is_some())
        .unwrap_or(false)
}

/// If `segment` is a `{name}` variable segment, returns its name (possibly
/// empty, e.g. for `{}`); otherwise returns `None`.
fn variable_name(segment: &str) -> Option<&str> {
    let bytes = segment.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'{' && bytes[bytes.len() - 1] == b'}' {
        Some(&segment[1..segment.len() - 1])
    } else {
        None
    }
}

/// Splits a resource name into its segments, skipping any `//service` full
/// resource name prefix and a single leading `/`. Mirrors the segment sequence
/// produced by aip-go's `Scanner` for names.
fn name_segments(name: &str) -> Vec<&str> {
    if name.is_empty() {
        return Vec::new();
    }
    let body = if let Some(rest) = name.strip_prefix("//") {
        // Full resource name: drop the service name (up to the next `/`).
        match rest.find('/') {
            Some(i) => &rest[i + 1..],
            None => return Vec::new(), // service name only, no resource segments
        }
    } else if let Some(rest) = name.strip_prefix('/') {
        rest
    } else {
        name
    };
    body.split('/').collect()
}

/// Returns the ancestor of `name` matching `pattern`, if any.
pub fn ancestor(_name: &str, _pattern: &str) -> Option<String> {
    todo!()
}

/// Reports whether `parent` is a parent resource name of `name`.
pub fn has_parent(_name: &str, _parent: &str) -> bool {
    todo!()
}

/// Reports whether `name` contains any wildcard (`-`) segments.
pub fn contains_wildcard(_name: &str) -> bool {
    todo!()
}

/// Validates that `name` is a well-formed resource name.
pub fn validate(_name: &str) -> Result<(), Error> {
    todo!()
}

/// Validates that `pattern` is a well-formed resource-name pattern.
pub fn validate_pattern(_pattern: &str) -> Result<(), Error> {
    todo!()
}

/// A single `/`-separated component of a resource name or pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment<'a>(pub &'a str);

impl<'a> Segment<'a> {
    /// Is this a `{variable}` segment?
    pub fn is_variable(&self) -> bool {
        let s = self.0.as_bytes();
        s.len() > 2 && s[0] == b'{' && s[s.len() - 1] == b'}'
    }

    /// Is this the wildcard segment `-`?
    pub fn is_wildcard(&self) -> bool {
        self.0 == WILDCARD
    }

    /// View this segment as a [`Literal`].
    pub fn literal(&self) -> Literal<'a> {
        Literal(self.0)
    }
}

/// A literal (fixed-value) segment, possibly carrying a revision after `@`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Literal<'a>(pub &'a str);

impl<'a> Literal<'a> {
    /// The resource ID, with any `@revision` stripped.
    pub fn resource_id(&self) -> &'a str {
        todo!()
    }

    /// The revision ID following `@`, if present.
    pub fn revision_id(&self) -> Option<&'a str> {
        todo!()
    }

    /// Does this literal carry a valid revision?
    pub fn has_revision(&self) -> bool {
        todo!()
    }
}

/// Iterates the segments of a resource name.
#[derive(Debug)]
pub struct Scanner<'a> {
    #[allow(dead_code)]
    rest: &'a str,
}

impl<'a> Scanner<'a> {
    /// Create a scanner over `name`.
    pub fn new(name: &'a str) -> Self {
        Self { rest: name }
    }

    /// Advance to and return the next [`Segment`], or `None` at the end.
    pub fn scan(&mut self) -> Option<Segment<'a>> {
        todo!()
    }
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_valid_and_rejects_malformed() {
        // (pattern, parses_ok)
        let cases = [
            ("shippers/{shipper}/sites/{site}", true),
            ("publishers", true),
            ("publishers/{publisher}/books/{book}/settings", true),
            ("/shippers/{shipper}", true), // a single leading slash is tolerated
            ("", false),                   // empty pattern
            ("/", false),                  // empty body after the slash
            ("//svc/shippers/{shipper}", false), // full resource name
            ("shippers/-/sites/-", false), // wildcard not allowed in a pattern
            ("shippers/{}", false),        // empty variable name
            ("shippers/{shipper", false),  // malformed variable (no closing brace)
            ("shippers/shipper}", false),  // malformed variable (no opening brace)
            ("a/{x}/b/{x}", false),        // duplicate variable name
            ("shippers//sites", false),    // empty interior segment
        ];
        for (pattern, ok) in cases {
            assert_eq!(
                Pattern::parse(pattern).is_ok(),
                ok,
                "Pattern::parse({pattern:?}).is_ok() should be {ok}"
            );
        }
    }

    #[test]
    fn is_match_table() {
        // Ported from aip-go resourcename `TestMatches`.
        // (pattern, name, expected)
        let cases = [
            (
                "shippers/{shipper}/sites/{site}",
                "shippers/1/sites/1",
                true,
            ),
            (
                "shippers/{shipper}/sites/{site}",
                "shippers/1/sites/1/settings",
                false, // name longer than pattern
            ),
            ("", "shippers/1/sites/1", false), // empty pattern
            ("", "", false),                   // empty pattern and empty name
            (
                "shippers/{shipper}/sites/{site}/settings",
                "shippers/1/sites/1/settings",
                true, // singleton
            ),
            ("shippers/-/sites/-", "shippers/1/sites/1", false), // wildcard pattern
            (
                "shippers/{shipper}/sites/{site}",
                "//freight-example.einride.tech/shippers/1/sites/1",
                true, // full parent name on the name side
            ),
            (
                "//freight-example.einride.tech/shippers/{shipper}",
                "shippers/1",
                false, // full resource name pattern
            ),
            ("shippers/{shipper}", "/shippers/1", true), // leading slash in the name
            ("/shippers/{shipper}", "shippers/1", true), // leading slash in the pattern
        ];
        for (pattern, name, expected) in cases {
            assert_eq!(
                is_match(pattern, name),
                expected,
                "is_match({pattern:?}, {name:?}) should be {expected}"
            );
        }
    }

    #[test]
    fn match_name_binds_variables() {
        // Ported from aip-go resourcename `TestScan` (named-captures form).
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        let caps = pattern.match_name("publishers/foo/books/bar").unwrap();
        assert_eq!(caps.get("publisher"), Some("foo"));
        assert_eq!(caps.get("book"), Some("bar"));
        assert_eq!(caps.get("missing"), None);

        // A pattern with no variables matches and binds nothing.
        let pattern = Pattern::parse("publishers").unwrap();
        let caps = pattern.match_name("publishers").unwrap();
        assert_eq!(caps.iter().count(), 0);

        // Singleton with trailing literal binds the interior variables.
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}/settings").unwrap();
        let caps = pattern
            .match_name("publishers/foo/books/bar/settings")
            .unwrap();
        assert_eq!(caps.get("publisher"), Some("foo"));
        assert_eq!(caps.get("book"), Some("bar"));
    }

    #[test]
    fn match_name_rejects_non_matches() {
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        // Trailing segments in the name.
        assert!(pattern
            .match_name("publishers/foo/books/bar/settings")
            .is_none());
        // Fewer segments than the pattern.
        assert!(pattern.match_name("publishers/foo").is_none());
        // Literal segment mismatch.
        assert!(pattern.match_name("shippers/foo/books/bar").is_none());
    }

    #[test]
    fn format_renders() {
        // Ported from aip-go resourcename `TestSprint` (the cases that map onto
        // the strict named API).
        let pattern = Pattern::parse("singleton").unwrap();
        assert_eq!(pattern.format([]).unwrap(), "singleton");

        let pattern = Pattern::parse("publishers/{publisher}").unwrap();
        assert_eq!(
            pattern.format([("publisher", "foo")]).unwrap(),
            "publishers/foo"
        );

        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        assert_eq!(
            pattern
                .format([("publisher", "foo"), ("book", "bar")])
                .unwrap(),
            "publishers/foo/books/bar"
        );

        let pattern = Pattern::parse("publishers/{publisher}/books/{book}/settings").unwrap();
        assert_eq!(
            pattern
                .format([("publisher", "foo"), ("book", "bar")])
                .unwrap(),
            "publishers/foo/books/bar/settings"
        );

        // An empty value is present (just empty), not missing.
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        assert_eq!(
            pattern
                .format([("publisher", "foo"), ("book", "")])
                .unwrap(),
            "publishers/foo/books/"
        );
    }

    #[test]
    fn format_errors_on_missing_and_unknown() {
        let pattern = Pattern::parse("publishers/{publisher}/books/{book}").unwrap();
        assert!(matches!(
            pattern.format([("publisher", "foo")]),
            Err(Error::MissingVariable { name }) if name == "book"
        ));

        let pattern = Pattern::parse("singleton").unwrap();
        assert!(matches!(
            pattern.format([("publisher", "foo")]),
            Err(Error::UnknownVariable { name }) if name == "publisher"
        ));
    }

    #[test]
    fn round_trip_format_of_match_reproduces_name() {
        // Acceptance: formatting a matched name's captures reproduces the name.
        let cases = [
            ("shippers/{shipper}/sites/{site}", "shippers/1/sites/1"),
            (
                "publishers/{publisher}/books/{book}",
                "publishers/foo/books/bar",
            ),
            (
                "publishers/{publisher}/books/{book}/settings",
                "publishers/foo/books/bar/settings",
            ),
            ("publishers", "publishers"),
        ];
        for (pattern, name) in cases {
            let pattern = Pattern::parse(pattern).unwrap();
            let caps = pattern.match_name(name).unwrap();
            assert_eq!(
                pattern.format(caps.iter()).unwrap(),
                name,
                "round-trip should reproduce {name:?}"
            );
        }
    }
}
