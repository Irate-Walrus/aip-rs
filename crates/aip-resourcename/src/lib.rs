//! AIP-122 resource names: parse, format, match, and validate.
//!
//! This crate is pure string work — it has no protobuf dependency.
//!
//! Unlike `aip-go`'s variadic `Sscan`/`Sprint`, parsing a name against a
//! [`Pattern`] yields named [`Captures`] (a `regex::Captures`-style API).
//! See `docs/adr/0002-idiomatic-resourcename-api.md`.
//!
//! See <https://google.aip.dev/122>.

use std::collections::BTreeMap;

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
}

/// The wildcard segment, `-` (matches any single resource ID).
pub const WILDCARD: &str = "-";
/// The revision separator, `@` (as in `books/les-miserables@1.0.0`).
pub const REVISION_SEPARATOR: char = '@';

/// A parsed resource-name pattern, e.g. `shippers/{shipper}/sites/{site}`.
#[derive(Debug, Clone)]
pub struct Pattern {
    #[allow(dead_code)]
    raw: String,
}

impl Pattern {
    /// Parse a pattern string into a reusable matcher.
    pub fn parse(_pattern: &str) -> Result<Self, Error> {
        todo!("split into segments; record variable positions and names")
    }

    /// Match a concrete resource name, binding each variable by name.
    pub fn match_name<'a>(&self, _name: &'a str) -> Option<Captures<'a>> {
        todo!("walk segments, bind variables, honour wildcards")
    }

    /// Format a resource name from `(variable, value)` pairs.
    pub fn format<'a, I>(&self, _vars: I) -> Result<String, Error>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        todo!("substitute variables into the pattern")
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
pub fn is_match(_pattern: &str, _name: &str) -> bool {
    todo!()
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
