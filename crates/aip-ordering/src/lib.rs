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
    /// - Paths may contain letters, digits, `_`, and `.` (for subfields).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(OrderBy::default());
        }

        // Validate characters up front: only letters, digits, '_', space, ',', '.'
        for c in s.chars() {
            if !c.is_alphabetic() && !c.is_numeric() && c != '_' && c != ' ' && c != ',' && c != '.'
            {
                return Err(Error::Syntax(format!(
                    "invalid character {}",
                    c.escape_debug()
                )));
            }
        }

        let mut fields = Vec::new();
        for raw in s.split(',') {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            match parts.as_slice() {
                [path] => fields.push(OrderByField {
                    path: (*path).to_owned(),
                    desc: false,
                }),
                [path, direction] => {
                    let desc = match *direction {
                        "asc" => false,
                        "desc" => true,
                        _ => return Err(Error::Syntax(format!("invalid format for '{raw}'"))),
                    };
                    fields.push(OrderByField {
                        path: (*path).to_owned(),
                        desc,
                    });
                }
                _ => return Err(Error::Syntax(format!("invalid format for '{raw}'"))),
            }
        }
        Ok(OrderBy { fields })
    }
}

impl OrderBy {
    /// Validates every field path against an explicit allow-list.
    pub fn validate_for_paths(&self, allowed: &[&str]) -> Result<(), Error> {
        for field in &self.fields {
            if !allowed.contains(&field.path.as_str()) {
                return Err(Error::UnknownField(field.path.clone()));
            }
        }
        Ok(())
    }

    /// Validates every field path against a message type (via `aip-fieldmask`).
    pub fn validate_for_message(&self, _descriptor: &MessageDescriptor) -> Result<(), Error> {
        todo!("convert paths to a FieldMask and call aip_fieldmask::validate")
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
