//! AIP-132 ordering: parse and validate `order_by`.
//!
//! Parsing and validation only — the sort itself is pushed to the datastore
//! (there is no in-memory sorter). Validation against a message reuses
//! `aip-fieldmask` for path resolution.
//!
//! See <https://google.aip.dev/132>.

use std::str::FromStr;

use prost_reflect::MessageDescriptor;

/// Errors produced when parsing or validating an `order_by`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid order_by syntax: {0}")]
    Syntax(String),
    #[error("unknown order_by field: {0}")]
    UnknownField(String),
}

/// A parsed AIP-132 ordering directive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderBy {
    /// The fields to order by, in priority order.
    pub fields: Vec<OrderByField>,
}

/// One field path plus direction within an [`OrderBy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderByField {
    /// The (possibly nested) field path, e.g. `author.name`.
    pub path: String,
    /// Descending if true, ascending otherwise.
    pub desc: bool,
}

impl OrderByField {
    /// The subfields of the path, split on `.`.
    ///
    /// Returns an empty iterator for an empty path (mirrors aip-go's `SubFields`
    /// returning nil), and one entry per `.`-separated segment otherwise.
    pub fn sub_fields(&self) -> impl Iterator<Item = &str> {
        (!self.path.is_empty())
            .then(|| self.path.split('.'))
            .into_iter()
            .flatten()
    }
}

impl FromStr for OrderBy {
    type Err = Error;

    /// Parses an AIP-132 `order_by` string.
    ///
    /// - An empty string is valid and yields an empty [`OrderBy`].
    /// - Fields are comma-separated; each field is `<path>` or `<path> asc|desc`.
    /// - Paths are ASCII identifiers optionally joined by `.` for subfields.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(OrderBy::default());
        }

        // Validate characters up front: only ASCII letters, digits, '_', space, ',', '.'
        for c in s.chars() {
            if !c.is_ascii_alphabetic()
                && !c.is_ascii_digit()
                && c != '_'
                && c != ' '
                && c != ','
                && c != '.'
            {
                return Err(Error::Syntax(format!(
                    "invalid order_by '{}': invalid character {}",
                    s,
                    c.escape_debug()
                )));
            }
        }

        let mut fields = Vec::new();
        for raw in s.split(',') {
            let mut parts = raw.split_whitespace();
            let Some(path) = parts.next() else {
                return Err(Error::Syntax(format!("invalid format for '{raw}'")));
            };
            validate_path_segments(path)?;
            let desc = match (parts.next(), parts.next()) {
                (None, _) => false,
                (Some("asc"), None) => false,
                (Some("desc"), None) => true,
                _ => return Err(Error::Syntax(format!("invalid format for '{raw}'"))),
            };
            fields.push(OrderByField {
                path: path.to_owned(),
                desc,
            });
        }
        Ok(OrderBy { fields })
    }
}

/// Validates that every `.`-separated segment of `path` is non-empty.
///
/// This rejects leading/trailing dots and consecutive dots (e.g. `"."`,
/// `".foo"`, `"foo."`, `"foo..bar"`), which the character allowlist permits
/// but which would produce empty segments when iterated by [`OrderByField::sub_fields`].
fn validate_path_segments(path: &str) -> Result<(), Error> {
    if path.split('.').any(|seg| seg.is_empty()) {
        return Err(Error::Syntax(format!("invalid path: '{path}'")));
    }
    Ok(())
}

impl OrderBy {
    /// Validates every field path against an explicit allow-list.
    ///
    /// Each entry in `allowed` must be the complete dot-notation path (e.g.
    /// `"book.name"`), not individual segments. Matching is exact string equality.
    pub fn validate_for_paths(&self, allowed: &[&str]) -> Result<(), Error> {
        for field in &self.fields {
            if !allowed.contains(&field.path.as_str()) {
                return Err(Error::UnknownField(field.path.clone()));
            }
        }
        Ok(())
    }

    /// Validates every field path against a message type (via `aip-fieldmask`).
    ///
    /// Each [`OrderByField`]'s path is checked against `descriptor` by reusing
    /// [`aip_fieldmask::validate`]: a path may descend through `.`-separated
    /// [`Subfields`] into nested message fields, and the first path that does not
    /// resolve yields [`Error::UnknownField`]. An empty [`OrderBy`] is valid
    /// against any message. Direction (asc/desc) is irrelevant to validation.
    ///
    /// [`Subfields`]: OrderByField::sub_fields
    pub fn validate_for_message(&self, descriptor: &MessageDescriptor) -> Result<(), Error> {
        let mask = prost_types::FieldMask {
            paths: self.fields.iter().map(|f| f.path.clone()).collect(),
        };
        aip_fieldmask::validate(&mask, descriptor).map_err(|err| match err {
            aip_fieldmask::Error::UnknownPath { path, .. } => Error::UnknownField(path),
            // Neither remaining variant is reachable for an `order_by`: the
            // full-replacement path `*` is rejected by the parser's character
            // allowlist (so never `WildcardNotAlone`), and `TypeMismatch` is
            // produced only by `update`, never `validate`. Surfaced defensively
            // rather than panicking should that ever change.
            other @ (aip_fieldmask::Error::WildcardNotAlone
            | aip_fieldmask::Error::TypeMismatch { .. }) => Error::Syntax(other.to_string()),
        })
    }
}

/// A request carrying an AIP-132 `order_by` string.
pub trait OrderByRequest {
    /// The `order_by` field of the request.
    fn order_by(&self) -> &str;
}

/// Parses the `order_by` from a request.
pub fn parse_order_by(request: &impl OrderByRequest) -> Result<OrderBy, Error> {
    request.order_by().parse()
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
    }
}
