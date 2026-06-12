//! aip-validation — a field-violation accumulator for AIP-193 error responses.
//!
//! A [`Validator`] collects per-field validation failures and resolves them into
//! a single [`Error`]. It is the generic surface a service reaches for when it
//! validates the fields no aip-rs primitive covers — its own presence and policy
//! checks — and wants the client to see *every* bad field in one response rather
//! than one error per round-trip.
//!
//! Ported from aip-go's [`validation`] package (`MessageValidator` / `Error`),
//! extended to also attach the mandatory `ErrorInfo`, whose `domain` and `reason`
//! identify *who* raised the error. Those are the caller's policy checks, so the
//! caller supplies the domain (its own service name) and reason — see
//! [`Validator::new`].
//!
//! [`validation`]: https://github.com/einride/aip-go/tree/main/validation
//!
//! ```
//! use aip_validation::Validator;
//!
//! let mut v = Validator::new("freight.example.com", "FIELD_REQUIRED");
//! v.add_field_violation("origin_site", "field is required");
//! v.add_field_violation("destination_site", "field is required");
//! // Both violations are reported together.
//! assert!(v.into_result().is_err());
//! ```
//!
//! Behind the `tonic` feature, [`Error`] converts to an `INVALID_ARGUMENT`
//! [`tonic::Status`] carrying a `BadRequest` with every violation and the
//! `ErrorInfo`.
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::fmt;

/// One bad field: the [field path](https://google.aip.dev/193) that failed and a
/// human-readable description of why.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldViolation {
    field: String,
    description: String,
}

/// Accumulates field violations and resolves them into one [`Error`].
///
/// Construct with [`Validator::new`], record failures with
/// [`add_field_violation`](Validator::add_field_violation), then call
/// [`into_result`](Validator::into_result): `Ok` when nothing was recorded, else
/// the aggregated [`Error`] carrying every violation.
///
/// To validate a nested message, prefix its field paths with
/// [`set_parent_field`](Validator::set_parent_field), or fold another validator's
/// [`Error`] in under a field with [`add_field_error`](Validator::add_field_error).
#[derive(Debug, Clone)]
pub struct Validator {
    domain: String,
    reason: String,
    parent_field: String,
    violations: Vec<FieldViolation>,
}

impl Validator {
    /// Creates an empty validator.
    ///
    /// `domain` and `reason` populate the AIP-193 `ErrorInfo` of the resolved
    /// [`Error`] (ADR-0007). `domain` identifies the service raising the error —
    /// its own service name, e.g. `freight.example.com`, not the `aip-rs`
    /// library domain. `reason` is the machine-readable, `UPPER_SNAKE_CASE`
    /// identifier for the aggregated failure (matching `[A-Z][A-Z0-9_]+[A-Z0-9]`);
    /// the per-field detail lives in each violation's description and the
    /// `BadRequest`.
    pub fn new(domain: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            reason: reason.into(),
            parent_field: String::new(),
            violations: Vec::new(),
        }
    }

    /// Sets a parent field prepended (with a `.`) to every subsequently added
    /// field path, so a nested message's validations land under their containing
    /// field. Persists until set again; pass `""` to clear it.
    pub fn set_parent_field(&mut self, parent_field: impl Into<String>) {
        self.parent_field = parent_field.into();
    }

    /// Records one field violation: `field` is the failing path (prefixed by the
    /// current [parent field](Validator::set_parent_field)), `description` says
    /// why.
    pub fn add_field_violation(&mut self, field: &str, description: impl Into<String>) {
        self.violations.push(FieldViolation {
            field: make_field_with_parent(&self.parent_field, field),
            description: description.into(),
        });
    }

    /// Folds another error's violations in under `field`.
    ///
    /// If `err` is a validation [`Error`], its individual violations are added
    /// with `field` (joined to any current parent) as their new parent — so a
    /// nested-message validator composes into its container. Any other error is
    /// recorded as a single violation on `field`, described by the error itself.
    pub fn add_field_error<E>(&mut self, field: &str, err: &E)
    where
        E: std::error::Error + 'static,
    {
        if let Some(inner) = (err as &dyn std::any::Any).downcast_ref::<Error>() {
            let nested_parent = make_field_with_parent(&self.parent_field, field);
            let original = std::mem::replace(&mut self.parent_field, nested_parent);
            for violation in &inner.field_violations {
                self.add_field_violation(&violation.field, violation.description.clone());
            }
            self.parent_field = original;
        } else {
            self.add_field_violation(field, err.to_string());
        }
    }

    /// Resolves the accumulated violations: `Ok(())` when none were recorded,
    /// otherwise the aggregated [`Error`].
    pub fn into_result(self) -> Result<(), Error> {
        if self.violations.is_empty() {
            Ok(())
        } else {
            Err(Error {
                domain: self.domain,
                reason: self.reason,
                field_violations: self.violations,
            })
        }
    }
}

/// Joins a parent field with a field name using `.`, or returns the field name
/// when there is no parent.
fn make_field_with_parent(parent_field: &str, field: &str) -> String {
    if parent_field.is_empty() {
        field.to_owned()
    } else {
        format!("{parent_field}.{field}")
    }
}

/// The aggregated result of a [`Validator`]: one or more field violations, plus
/// the `domain` and `reason` for the AIP-193 `ErrorInfo`.
///
/// Only ever produced by [`Validator::into_result`], which guarantees at least
/// one violation — there is no empty `Error`.
#[derive(Debug, Clone)]
pub struct Error {
    domain: String,
    reason: String,
    field_violations: Vec<FieldViolation>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let [only] = self.field_violations.as_slice() {
            write!(f, "field violation on {}: {}", only.field, only.description)
        } else {
            write!(f, "field violation on multiple fields:")?;
            for violation in &self.field_violations {
                write!(f, "\n | {}: {}", violation.field, violation.description)?;
            }
            Ok(())
        }
    }
}

impl std::error::Error for Error {}

/// The AIP-193 `ErrorInfo.domain` for a service's own checks comes from the
/// [`Validator`] (see [`Validator::new`]); this crate hard-codes none. The
/// `tonic` mapping below threads the caller-supplied `domain`/`reason` straight
/// into the `ErrorInfo`.
#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: a `BadRequest`
    /// listing **every** accumulated violation at once, and the mandatory
    /// `ErrorInfo` carrying the caller's `reason` + `domain` with the offending
    /// field paths mirrored under the `fields` metadata key (so a machine actor
    /// reads them without parsing the message). See
    /// `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let fields = err
            .field_violations
            .iter()
            .map(|violation| violation.field.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let message = format!("invalid fields: {fields}");

        let mut details = ErrorDetails::new();
        details.set_error_info(
            err.reason,
            err.domain,
            HashMap::from([("fields".to_owned(), fields)]),
        );
        for violation in err.field_violations {
            details.add_bad_request_violation(violation.field, violation.description);
        }
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_violation_is_ok() {
        let v = Validator::new("freight.example.com", "FIELD_VIOLATION");
        assert!(v.into_result().is_ok());
    }

    #[test]
    fn single_violation() {
        let mut v = Validator::new("freight.example.com", "FIELD_VIOLATION");
        v.add_field_violation("foo", "bar");
        let err = v.into_result().expect_err("a violation was recorded");
        assert_eq!(err.to_string(), "field violation on foo: bar");
    }

    #[test]
    fn single_violation_with_parent() {
        let mut v = Validator::new("freight.example.com", "FIELD_VIOLATION");
        v.set_parent_field("foo");
        v.add_field_violation("bar", "baz");
        let err = v.into_result().expect_err("a violation was recorded");
        assert_eq!(err.to_string(), "field violation on foo.bar: baz");
    }

    #[test]
    fn multiple_violations() {
        let mut v = Validator::new("freight.example.com", "FIELD_VIOLATION");
        v.add_field_violation("foo.bar", "test");
        v.add_field_violation("baz", "test2");
        let err = v.into_result().expect_err("violations were recorded");
        assert_eq!(
            err.to_string(),
            "field violation on multiple fields:\n | foo.bar: test\n | baz: test2"
        );
    }

    #[test]
    fn nested_violations_fold_under_parent() {
        let mut inner = Validator::new("freight.example.com", "FIELD_VIOLATION");
        inner.add_field_violation("b", "c");
        let inner_err = inner.into_result().expect_err("a violation was recorded");

        let mut outer = Validator::new("freight.example.com", "FIELD_VIOLATION");
        outer.add_field_error("a", &inner_err);
        let err = outer
            .into_result()
            .expect_err("the folded violation remains");
        assert_eq!(err.to_string(), "field violation on a.b: c");
    }

    #[test]
    fn add_field_error_with_plain_error() {
        #[derive(Debug)]
        struct Boom;
        impl fmt::Display for Boom {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("boom")
            }
        }
        impl std::error::Error for Boom {}

        let mut v = Validator::new("freight.example.com", "FIELD_VIOLATION");
        v.add_field_error("a", &Boom);
        let err = v.into_result().expect_err("a violation was recorded");
        assert_eq!(err.to_string(), "field violation on a: boom");
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn maps_all_violations_to_one_bad_request() {
        let mut v = Validator::new("freight.example.com", "FIELD_REQUIRED");
        v.add_field_violation("foo.bar", "test");
        v.add_field_violation("baz", "test2");
        let status: tonic::Status = v.into_result().expect_err("violations recorded").into();

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(status.message(), "invalid fields: foo.bar, baz");

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest is attached (AIP-193)");
        assert_eq!(bad.field_violations.len(), 2);
        assert_eq!(bad.field_violations[0].field, "foo.bar");
        assert_eq!(bad.field_violations[0].description, "test");
        assert_eq!(bad.field_violations[1].field, "baz");
        assert_eq!(bad.field_violations[1].description, "test2");
    }

    #[test]
    fn error_info_carries_caller_domain_and_reason() {
        let mut v = Validator::new("freight.example.com", "FIELD_REQUIRED");
        v.add_field_violation("origin_site", "field is required");
        let status: tonic::Status = v.into_result().expect_err("violation recorded").into();

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, "freight.example.com");
        // The offending field paths are mirrored in metadata (ADR-0007).
        assert_eq!(
            info.metadata.get("fields").map(String::as_str),
            Some("origin_site")
        );
    }
}
