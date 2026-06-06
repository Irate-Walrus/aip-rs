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
    pub fn sub_fields(&self) -> impl Iterator<Item = &str> {
        self.path.split('.')
    }
}

impl FromStr for OrderBy {
    type Err = Error;

    fn from_str(_s: &str) -> Result<Self, Self::Err> {
        todo!("parse comma-separated fields with optional asc/desc")
    }
}

impl OrderBy {
    /// Validates every field path against an explicit allow-list.
    pub fn validate_for_paths(&self, _allowed: &[&str]) -> Result<(), Error> {
        todo!()
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
