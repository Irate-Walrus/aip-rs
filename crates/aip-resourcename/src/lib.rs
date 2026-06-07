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
    #[error("segment {segment:?}: resource names must not contain variables")]
    VariableInName { segment: String },
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

/// Collects the segments of a resource name via [`Scanner`], skipping any
/// `//service` prefix and a single leading `/`.
fn name_segments(name: &str) -> Vec<&str> {
    let mut scanner = Scanner::new(name);
    let mut segments = Vec::new();
    while let Some(segment) = scanner.scan() {
        segments.push(segment.0);
    }
    segments
}

/// Extracts the ancestor of `name` selected by `pattern`, if `name` matches.
///
/// Walks `pattern` and `name` in lockstep; literal segments must be equal and
/// variable segments bind any segment. Returns the prefix of `name` covered by
/// the pattern (including any `//service` prefix), or `None` if `name` does not
/// match, either is empty, or `pattern` contains a [`Wildcard`](WILDCARD).
pub fn ancestor(name: &str, pattern: &str) -> Option<String> {
    if name.is_empty() || pattern.is_empty() {
        return None;
    }
    let mut name_scanner = Scanner::new(name);
    let mut pattern_scanner = Scanner::new(pattern);
    while let Some(pattern_segment) = pattern_scanner.scan() {
        let name_segment = name_scanner.scan()?;
        if pattern_segment.is_wildcard() {
            return None; // wildcards are not allowed in patterns
        }
        if !pattern_segment.is_variable() && pattern_segment != name_segment {
            return None; // literal mismatch
        }
    }
    Some(name[..name_scanner.end].to_string())
}

/// Reports whether `parent` is a parent resource name of `name`.
///
/// [`Wildcard`](WILDCARD) (`-`) segments in `parent` match any segment. A
/// resource name without a revision is a parent of the same name carrying a
/// revision (per AIP-162). Full resource names match only when their service
/// names agree.
pub fn has_parent(name: &str, parent: &str) -> bool {
    if name.is_empty() || parent.is_empty() || name == parent {
        return false;
    }
    let mut parent_scanner = Scanner::new(parent);
    let mut name_scanner = Scanner::new(name);
    while let Some(parent_segment) = parent_scanner.scan() {
        let Some(name_segment) = name_scanner.scan() else {
            return false;
        };
        if parent_segment.is_wildcard() {
            continue;
        }
        // A non-revisioned resource ID is the parent of its revisioned form.
        if name_segment.literal().has_revision()
            && !parent_segment.literal().has_revision()
            && name_segment.literal().resource_id() == parent_segment.literal().resource_id()
        {
            continue;
        }
        if parent_segment != name_segment {
            return false;
        }
    }
    if parent_scanner.full() && name_scanner.full() {
        return parent_scanner.service_name() == name_scanner.service_name();
    }
    true
}

/// Reports whether `name` contains any wildcard (`-`) segments.
pub fn contains_wildcard(name: &str) -> bool {
    let mut scanner = Scanner::new(name);
    while let Some(segment) = scanner.scan() {
        if segment.is_wildcard() {
            return true;
        }
    }
    false
}

/// Validates that `name` is a well-formed resource name per AIP-122.
///
/// Each segment must be a [`Wildcard`](WILDCARD) or a valid DNS-name
/// [`Literal`]; variables are not allowed. A `//service` prefix must be a valid
/// DNS name.
pub fn validate(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Empty);
    }
    let mut scanner = Scanner::new(name);
    let mut index = 0;
    while let Some(segment) = scanner.scan() {
        index += 1;
        if segment.0.is_empty() {
            return Err(Error::EmptySegment { index });
        }
        if segment.is_wildcard() {
            continue;
        }
        if segment.is_variable() {
            return Err(Error::VariableInName {
                segment: segment.0.to_string(),
            });
        }
        if !is_domain_name(segment.0) {
            return Err(Error::InvalidDnsName {
                segment: segment.0.to_string(),
            });
        }
    }
    if scanner.full() && !is_domain_name(scanner.service_name()) {
        return Err(Error::InvalidDnsName {
            segment: scanner.service_name().to_string(),
        });
    }
    Ok(())
}

/// Validates that `pattern` is a well-formed resource-name pattern per AIP-122.
///
/// Each segment must be a valid DNS-name [`Literal`] or a `{snake_case}`
/// variable. Wildcards and full resource names are rejected.
pub fn validate_pattern(pattern: &str) -> Result<(), Error> {
    if pattern.is_empty() {
        return Err(Error::InvalidPattern("pattern is empty".to_string()));
    }
    let mut scanner = Scanner::new(pattern);
    let mut index = 0;
    while let Some(segment) = scanner.scan() {
        index += 1;
        if segment.0.is_empty() {
            return Err(Error::EmptySegment { index });
        }
        if segment.is_wildcard() {
            return Err(Error::InvalidPattern(
                "wildcards not allowed in patterns".to_string(),
            ));
        }
        if segment.is_variable() {
            let name = segment.literal().0;
            if name.is_empty() {
                return Err(Error::InvalidPattern("missing variable name".to_string()));
            }
            if !is_snake_case(name) {
                return Err(Error::InvalidPattern(
                    "variable name must be valid snake case".to_string(),
                ));
            }
        } else if !is_domain_name(segment.0) {
            return Err(Error::InvalidDnsName {
                segment: segment.0.to_string(),
            });
        }
    }
    if scanner.full() {
        return Err(Error::InvalidPattern(
            "patterns can not be full resource names".to_string(),
        ));
    }
    Ok(())
}

/// Reports whether `s` is a valid snake-case identifier: a lowercase letter
/// followed by lowercase letters, digits, or underscores.
fn is_snake_case(s: &str) -> bool {
    for (i, c) in s.chars().enumerate() {
        if i == 0 {
            if !c.is_lowercase() {
                return false;
            }
        } else if c != '_' && !c.is_lowercase() && !c.is_numeric() {
            return false;
        }
    }
    true
}

/// Reports whether `s` is a valid DNS name (RFC 1035 / RFC 3696). Ported from
/// Go's `net.isDomainName`, as aip-go does.
fn is_domain_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    let l = bytes.len();
    if l == 0 || l > 254 || (l == 254 && bytes[l - 1] != b'.') {
        return false;
    }
    let mut last = b'.';
    let mut part_len = 0;
    for &c in bytes {
        match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' => part_len += 1,
            b'-' => {
                // A byte before a dash cannot be a dot.
                if last == b'.' {
                    return false;
                }
                part_len += 1;
            }
            b'.' => {
                // A byte before a dot cannot be a dot or dash.
                if last == b'.' || last == b'-' {
                    return false;
                }
                if part_len > 63 || part_len == 0 {
                    return false;
                }
                part_len = 0;
            }
            _ => return false,
        }
        last = c;
    }
    if last == b'-' || part_len > 63 {
        return false;
    }
    true
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

    /// View this segment as a [`Literal`]. For a `{variable}` segment, the
    /// literal value is the variable name (with the braces stripped).
    pub fn literal(&self) -> Literal<'a> {
        if self.is_variable() {
            Literal(&self.0[1..self.0.len() - 1])
        } else {
            Literal(self.0)
        }
    }
}

/// A literal (fixed-value) segment, possibly carrying a revision after `@`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Literal<'a>(pub &'a str);

impl<'a> Literal<'a> {
    /// The resource ID, with any `@revision` stripped.
    pub fn resource_id(&self) -> &'a str {
        match self.0.find(REVISION_SEPARATOR) {
            Some(i) if self.has_revision() => &self.0[..i],
            _ => self.0,
        }
    }

    /// The revision ID following `@`, if present.
    pub fn revision_id(&self) -> Option<&'a str> {
        match self.0.find(REVISION_SEPARATOR) {
            // `@` is one byte, so `i + 1` is a valid boundary.
            Some(i) if self.has_revision() => Some(&self.0[i + 1..]),
            _ => None,
        }
    }

    /// Does this literal carry a valid revision? A valid revision has non-empty
    /// content on each side of a single `@`.
    pub fn has_revision(&self) -> bool {
        let Some(i) = self.0.find(REVISION_SEPARATOR) else {
            return false;
        };
        if i < 1 || i >= self.0.len() - 1 {
            return false; // content required on each side of `@`
        }
        // A second `@` means there is no single, valid revision.
        !self.0[i + 1..].contains(REVISION_SEPARATOR)
    }
}

/// Iterates the [`Segment`]s of a resource name or pattern.
///
/// A leading `//service` prefix (a full resource name) is recognised: its
/// service name is exposed via [`service_name`](Scanner::service_name) and
/// [`full`](Scanner::full) reports `true`, while [`scan`](Scanner::scan) yields
/// only the resource segments. A single leading `/` is skipped.
#[derive(Debug)]
pub struct Scanner<'a> {
    name: &'a str,
    /// Start byte index (inclusive) of the current segment.
    start: usize,
    /// End byte index (exclusive) of the current segment.
    end: usize,
    service_start: usize,
    service_end: usize,
    full: bool,
}

impl<'a> Scanner<'a> {
    /// Create a scanner over `name`.
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            start: 0,
            end: 0,
            service_start: 0,
            service_end: 0,
            full: false,
        }
    }

    /// Advance to and return the next [`Segment`], or `None` at the end.
    pub fn scan(&mut self) -> Option<Segment<'a>> {
        let len = self.name.len();
        if self.end == len {
            return None;
        }
        if self.end == 0 {
            // First scan: handle full resource names and a leading slash.
            if self.name.starts_with("//") {
                self.full = true;
                self.start = 2;
                match self.name[2..].find('/') {
                    None => {
                        // Service name with no resource segments.
                        self.service_start = 2;
                        self.service_end = len;
                        self.start = len;
                        self.end = len;
                        return None;
                    }
                    Some(next) => {
                        self.service_start = 2;
                        self.service_end = 2 + next;
                        self.start = 2 + next + 1;
                    }
                }
            } else if self.name.starts_with('/') {
                self.start = 1; // skip the leading slash
            }
        } else {
            self.start = self.end + 1; // skip the slash ending the last segment
        }
        self.end = match self.name[self.start..].find('/') {
            Some(next) => self.start + next,
            None => len,
        };
        Some(Segment(&self.name[self.start..self.end]))
    }

    /// Whether the scanned name is a full resource name (`//service/...`).
    pub fn full(&self) -> bool {
        self.full
    }

    /// The service name of a full resource name, or `""` otherwise.
    pub fn service_name(&self) -> &'a str {
        &self.name[self.service_start..self.service_end]
    }
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain. See `docs/adr/0007-aip193-error-details.md`.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `reason` + [`domain`](ERROR_DOMAIN) and the
    /// error's dynamic values as `metadata`. A resource name is an opaque value
    /// rather than a request field path, so no `BadRequest` is attached.
    /// See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::Empty => ("RESOURCE_NAME_EMPTY", HashMap::new()),
            Error::EmptySegment { index } => (
                "RESOURCE_NAME_EMPTY_SEGMENT",
                HashMap::from([("index".to_owned(), index.to_string())]),
            ),
            Error::InvalidDnsName { segment } => (
                "RESOURCE_NAME_INVALID_SEGMENT",
                HashMap::from([("segment".to_owned(), segment.clone())]),
            ),
            Error::PatternMismatch { pattern } => (
                "RESOURCE_NAME_PATTERN_MISMATCH",
                HashMap::from([("pattern".to_owned(), pattern.clone())]),
            ),
            Error::InvalidPattern(_) => ("RESOURCE_NAME_INVALID_PATTERN", HashMap::new()),
            Error::VariableInName { segment } => (
                "RESOURCE_NAME_VARIABLE_IN_NAME",
                HashMap::from([("segment".to_owned(), segment.clone())]),
            ),
            Error::MissingVariable { name } => (
                "RESOURCE_NAME_MISSING_VARIABLE",
                HashMap::from([("variable".to_owned(), name.clone())]),
            ),
            Error::UnknownVariable { name } => (
                "RESOURCE_NAME_UNKNOWN_VARIABLE",
                HashMap::from([("variable".to_owned(), name.clone())]),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn maps_to_invalid_argument_with_error_info_and_metadata() {
        let status: tonic::Status = Error::InvalidDnsName {
            segment: "Bad_Seg".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "RESOURCE_NAME_INVALID_SEGMENT");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("segment").map(String::as_str),
            Some("Bad_Seg"),
        );

        // A resource name is an opaque value, not a request field path, so there
        // is no BadRequest.
        assert!(status.get_details_bad_request().is_none());
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

    fn assert_err_contains(result: Result<(), Error>, needle: &str) {
        match result {
            Ok(()) => panic!("expected an error containing {needle:?}, got Ok"),
            Err(e) => assert!(
                e.to_string().contains(needle),
                "error {e:?} should contain {needle:?}"
            ),
        }
    }

    #[test]
    fn scanner_iterates_segments() {
        // Ported from aip-go `TestSegmentScanner`.
        // (input, full, service_name, segments)
        let cases: &[(&str, bool, &str, &[&str])] = &[
            ("", false, "", &[]),
            ("singleton", false, "", &["singleton"]),
            ("shippers/1", false, "", &["shippers", "1"]),
            (
                "shippers/1/settings",
                false,
                "",
                &["shippers", "1", "settings"],
            ),
            (
                "shippers/1/shipments/-",
                false,
                "",
                &["shippers", "1", "shipments", "-"],
            ),
            (
                "shippers//shipments",
                false,
                "",
                &["shippers", "", "shipments"],
            ),
            ("shippers/", false, "", &["shippers", ""]),
            (
                "//library.googleapis.com/publishers/123/books/les-miserables",
                true,
                "library.googleapis.com",
                &["publishers", "123", "books", "les-miserables"],
            ),
            (
                "//library.googleapis.com",
                true,
                "library.googleapis.com",
                &[],
            ),
            ("//", true, "", &[]),
        ];
        for (input, full, service, segments) in cases {
            let mut scanner = Scanner::new(input);
            let mut got = Vec::new();
            while let Some(segment) = scanner.scan() {
                got.push(segment.0);
            }
            assert_eq!(scanner.full(), *full, "full for {input:?}");
            assert_eq!(scanner.service_name(), *service, "service for {input:?}");
            assert_eq!(got.as_slice(), *segments, "segments for {input:?}");
        }
    }

    #[test]
    fn validate_table() {
        // Ported from aip-go `TestValidate`.
        for ok in [
            "foo",
            "-",
            "foo/bar",
            "-/bar",
            "foo/-",
            "foo/-/bar",
            "foo/1234/bar",
            "FOO/1234/bAr",
            "//example.com/foo/bar",
        ] {
            assert!(validate(ok).is_ok(), "{ok:?} should validate");
        }
        assert_err_contains(validate(""), "empty");
        assert_err_contains(validate("ice cream is best"), "not a valid DNS name");
        assert_err_contains(
            validate("foo/bar/ice cream is best"),
            "not a valid DNS name",
        );
        assert_err_contains(
            validate("//ice cream is best.com/foo/bar"),
            "not a valid DNS name",
        );
        assert_err_contains(validate("foo/bar/{baz}"), "must not contain variables");
    }

    #[test]
    fn validate_pattern_table() {
        // Ported from aip-go `TestValidatePattern`.
        for ok in [
            "foo/bar/{baz}",
            "foo",
            "foo/bar",
            "foo/1234/bar",
            "FOO/1234/bAr",
            "fooBars/{foo_bar}",
        ] {
            assert!(validate_pattern(ok).is_ok(), "{ok:?} should validate");
        }
        assert_err_contains(validate_pattern(""), "empty");
        assert_err_contains(
            validate_pattern("ice cream is best"),
            "not a valid DNS name",
        );
        assert_err_contains(
            validate_pattern("foo/bar/ice cream is best"),
            "not a valid DNS name",
        );
        assert_err_contains(
            validate_pattern("//ice cream is best.com/foo/bar"),
            "patterns can not be full resource names",
        );
        for wildcard in ["-", "-/bar", "foo/-", "foo/-/bar"] {
            assert_err_contains(
                validate_pattern(wildcard),
                "wildcards not allowed in patterns",
            );
        }
        assert_err_contains(
            validate_pattern("//example.com/foo/bar"),
            "patterns can not be full resource names",
        );
        assert_err_contains(validate_pattern("fooBars/{fooBar}"), "snake case");
    }

    #[test]
    fn ancestor_table() {
        // Ported from aip-go `TestAncestor`.
        assert_eq!(ancestor("", ""), None);
        assert_eq!(ancestor("foo/1/bar/2", ""), None);
        assert_eq!(ancestor("", "foo/{foo}"), None);
        assert_eq!(ancestor("foo/1/bar/2", "baz/{baz}"), None);
        assert_eq!(
            ancestor("foo/1/bar/2", "foo/{foo}").as_deref(),
            Some("foo/1")
        );
        assert_eq!(
            ancestor("//foo.example.com/foo/1/bar/2", "foo/{foo}").as_deref(),
            Some("//foo.example.com/foo/1")
        );
    }

    #[test]
    fn has_parent_table() {
        // Ported from aip-go `TestHasParent`. (name, parent, expected)
        let cases = [
            ("shippers/1/sites/1", "shippers/1", true),
            (
                "shippers/1/sites/1/settings",
                "shippers/1/sites/1/settings",
                false,
            ),
            ("shippers/1/sites/1", "", false),
            ("", "", false),
            ("shippers/1/settings", "shippers/1", true),
            ("shippers/1/sites/1/settings", "shippers/1/sites/1", true),
            ("shippers/1/sites/1", "shippers/-", true),
            (
                "//freight-example.einride.tech/shippers/1/sites/1",
                "shippers/-",
                true,
            ),
            (
                "shippers/1/sites/1",
                "//freight-example.einride.tech/shippers/-",
                true,
            ),
            (
                "//other-example.einride.tech/shippers/1/sites/1",
                "//freight-example.einride.tech/shippers/-",
                false,
            ),
            ("shippers/1/sites/1@beef", "shippers/1/sites/1", true),
            ("shippers/1/sites/1@beef", "shippers/1/sites/1@dead", false),
            ("shippers/1/sites/1@beef", "shippers/1/sites/1@beef", false),
            ("datasets/1@beef/tables/1", "datasets/1@beef", true),
            ("datasets/1/tables/1", "datasets/1@beef", false),
            ("datasets/1@dead/tables/1", "datasets/1@beef", false),
            ("datasets/1@beef/tables/1", "datasets/1", true),
        ];
        for (name, parent, expected) in cases {
            assert_eq!(
                has_parent(name, parent),
                expected,
                "has_parent({name:?}, {parent:?})"
            );
        }
    }

    #[test]
    fn contains_wildcard_table() {
        // Ported from aip-go `TestContainsWildcard`.
        for (input, expected) in [
            ("", false),
            ("foo", false),
            ("-", true),
            ("foo/bar", false),
            ("-/bar", true),
            ("foo/-", true),
            ("foo/-/bar", true),
        ] {
            assert_eq!(
                contains_wildcard(input),
                expected,
                "contains_wildcard({input:?})"
            );
        }
    }

    #[test]
    fn literal_revision_parsing() {
        let revisioned = Literal("les-miserables@1.0.0");
        assert!(revisioned.has_revision());
        assert_eq!(revisioned.resource_id(), "les-miserables");
        assert_eq!(revisioned.revision_id(), Some("1.0.0"));

        let plain = Literal("les-miserables");
        assert!(!plain.has_revision());
        assert_eq!(plain.resource_id(), "les-miserables");
        assert_eq!(plain.revision_id(), None);

        // `@` at an edge, or doubled, is not a valid revision.
        for invalid in ["@1.0.0", "book@", "a@b@c"] {
            let literal = Literal(invalid);
            assert!(!literal.has_revision(), "{invalid:?} has no valid revision");
            assert_eq!(literal.resource_id(), invalid);
            assert_eq!(literal.revision_id(), None);
        }
    }

    #[test]
    fn segment_literal_strips_variable_braces() {
        assert_eq!(Segment("{shipper}").literal().0, "shipper");
        assert_eq!(Segment("shippers").literal().0, "shippers");
        assert_eq!(Segment("-").literal().0, "-");
    }
}
