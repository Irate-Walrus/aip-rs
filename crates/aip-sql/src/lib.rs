//! aip-sql: transpile an AIP-160 [`Filter`] into a parameterized, dialect-rendered
//! SQL [`Predicate`].
//!
//! The native Filter AST (ADR-0003) is the integration point: [`transpile_filter`]
//! walks it into a small, composable boolean [`Predicate`] whose logical structure
//! (`AND`/`OR`/`NOT`) is portable and whose leaves are *spelled* by a [`Dialect`].
//! A single [`Dialect::render`] pass turns a `Predicate` into `(sql, Vec<Value>)` —
//! SQL text plus an ordered list of [bound `Value`s](Value) — numbering every
//! placeholder left-to-right and parenthesizing by precedence.
//!
//! The cardinal rule (ADR-0005 / ADR-0008): **parameterize, never interpolate.**
//! A filter is attacker-controlled, so every literal becomes a bound [`Value`],
//! never spliced into SQL text. This crate depends on no datastore — the caller
//! binds the values to whatever driver it uses.
//!
//! [`transpile_filter`] lowers the full AIP-160 operator set the checker
//! accepts: the comparisons `=` / `!=` / `<` / `<=` / `>` / `>=`, the logical
//! `AND` / `OR` / `NOT`, member access into `map` columns (`labels.env`), the
//! `timestamp(...)` / `duration(...)` constructors, and the has operator `:`
//! (substring, map-key / list-element membership, and timestamp presence — the
//! per-engine [`Dialect`] leaves). Because [`check`] yields an *untyped*
//! expression tree, it is handed the [`Declarations`] and a column [`Schema`] to
//! recover each operand's type and map each identifier to a column (ADR-0008).
//! See `docs/adr/0008-aip-sql-predicate-dialect.md`.
//!
//! [`transpile_order_by`] is the ordering counterpart: it maps an AIP-132
//! [`OrderBy`](aip_ordering::OrderBy)'s field paths onto SQL `ORDER BY`
//! [items](Order) through the same column [`Schema`].
//!
//! [`Query`] reunites the two halves: it bundles the WHERE [`Predicate`], the
//! `ORDER BY` [items](Order), and an AIP-158 page token's offset and size, and
//! [`Query::render`] spells the whole `WHERE … ORDER BY … LIMIT … OFFSET` clause
//! tail plus its binds in one call — so a caller makes a single call instead of
//! stitching the filter and `order_by` halves together by hand. Only the WHERE
//! binds; the `order_by` and the page integers carry no attacker-controlled
//! literals. The `SELECT` / `FROM` head stays the caller's.
//!
//! [`check`]: aip_filtering::check
//! [`Declarations`]: aip_filtering::Declarations

mod dialect;
mod order;
mod predicate;
mod query;
mod schema;
mod timestamp;
mod transpile;

pub use dialect::{Dialect, Sqlite};
pub use order::{transpile_order_by, Order};
pub use predicate::{CmpOp, Column, HasTest, Predicate, Value};
pub use query::Query;
pub use schema::Schema;
pub use timestamp::format_timestamp;
pub use transpile::transpile_filter;

/// Errors produced when transpiling a [`Filter`](aip_filtering::Filter) into a
/// [`Predicate`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filter construct this transpiler does not handle (e.g. a comparison
    /// between two columns).
    #[error("unsupported filter construct: {0}")]
    Unsupported(String),
    /// A filter identifier or `order_by` field path with no column mapping in the
    /// [`Schema`] — it is not a filterable/sortable column.
    #[error("filter identifier `{0}` is not a filterable column")]
    UnknownIdentifier(String),
    /// A `duration(...)` literal that is not a number of seconds (e.g. `"10m"`).
    /// The checker accepts any string argument, so the format is validated here.
    #[error("invalid duration literal `{0}`: expected a number of seconds like `3600s`")]
    InvalidDuration(String),
}

/// The library-internal AIP-193 `ErrorInfo.domain` the user-fault errors this
/// crate maps are stamped with. It is a sentinel meaning "replace at the serving
/// boundary": a deploying service installs the `aip-errordomain` layer, which
/// rewrites it to the service's own domain so clients see one domain (ADR-0007).
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to a [`tonic::Status`], encoding user-vs-server fault by variant
    /// (ADR-0007 / ADR-0008) so the call site needs no hand-rolled `format!`:
    ///
    /// - [`Unsupported`](Error::Unsupported) and
    ///   [`InvalidDuration`](Error::InvalidDuration) are bad client input — an
    ///   unlowerable filter construct or a malformed `duration(...)` literal — so
    ///   they map to `INVALID_ARGUMENT` with an AIP-193 `ErrorInfo` under the
    ///   library sentinel [`ERROR_DOMAIN`] (`aip-rs`), which a deploying service
    ///   rewrites to its own domain at the serving boundary with the
    ///   `aip-errordomain` layer. A filter expression / duration literal is an
    ///   opaque value the library validates without knowing which request field
    ///   carried it, so it gets an `ErrorInfo` only — no `BadRequest` (ADR-0007).
    /// - [`UnknownIdentifier`](Error::UnknownIdentifier) means an identifier
    ///   passed the checker / `order_by` allow-list but has no column in the
    ///   [`Schema`] — a server-side Schema/allow-list drift, never client input
    ///   (ADR-0008) — so it maps to `INTERNAL` (no domain, nothing to rewrite).
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::Unsupported(detail) => (
                "FILTER_UNSUPPORTED",
                HashMap::from([("detail".to_owned(), detail.clone())]),
            ),
            Error::InvalidDuration(literal) => (
                "FILTER_INVALID_DURATION",
                HashMap::from([("literal".to_owned(), literal.clone())]),
            ),
            // Schema/allow-list drift is a server bug, not bad input (ADR-0008):
            // an `INTERNAL` carrying no machine-readable details.
            Error::UnknownIdentifier(_) => return tonic::Status::internal(message),
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
    fn unsupported_maps_to_invalid_argument_with_error_info() {
        let status: tonic::Status = Error::Unsupported("operator `foo`".to_owned()).into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "FILTER_UNSUPPORTED");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("detail").map(String::as_str),
            Some("operator `foo`")
        );
        // A filter expression is opaque — ErrorInfo only, no BadRequest (ADR-0007).
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn invalid_duration_maps_to_invalid_argument_with_error_info() {
        let status: tonic::Status = Error::InvalidDuration("10m".to_owned()).into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "FILTER_INVALID_DURATION");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("literal").map(String::as_str),
            Some("10m")
        );
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn unknown_identifier_maps_to_internal() {
        // Schema/allow-list drift is a server bug, not client input (ADR-0008).
        let status: tonic::Status = Error::UnknownIdentifier("display_name".to_owned()).into();
        assert_eq!(status.code(), tonic::Code::Internal);
        // No AIP-193 details leak on an internal error.
        assert!(status.get_details_error_info().is_none());
    }
}
