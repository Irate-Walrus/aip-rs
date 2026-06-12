//! AIP-155 request identification: validate a `request_id` and name the
//! idempotency (de-duplication) contract.
#![cfg_attr(docsrs, feature(doc_cfg))]
//!
//! A `request_id` makes a mutating call idempotent: a retry carrying the same
//! id returns the original result instead of performing the action twice (so a
//! create does not mint a second resource). This crate stays at the library's
//! parse-and-validate boundary — it validates the id and names the three
//! [`Replay`] outcomes of looking one up. The cache of seen ids, and the stored
//! responses, belong to the caller (a server).
//!
//! Per AIP-155 the id is optional ("Request IDs should be optional") and its
//! format is a UUID — the field carries the `(google.api.field_info).format =
//! UUID4` annotation. [`validate`] treats an empty id as "no idempotency
//! requested" and accepts any parseable UUID for a non-empty one, matching the
//! lenient `aip-resourceid` parse rather than insisting on version 4.
//!
//! Pure string plus UUID work; no protobuf dependency.
//!
//! See <https://google.aip.dev/155>.

/// Error validating a `request_id`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The id is non-empty but does not parse as a UUID.
    #[error("request_id must be a UUID (got {len} characters)")]
    NotAUuid {
        /// The length of the offending id, mirrored into AIP-193 `metadata`.
        len: usize,
    },
}

/// Validates an AIP-155 `request_id`.
///
/// An empty id means no idempotency was requested and always validates; a
/// non-empty id must parse as a UUID. Lenient about the UUID version — the field
/// advertises UUID4, but any UUID is accepted, matching `aip-resourceid`.
pub fn validate(request_id: &str) -> Result<(), Error> {
    if request_id.is_empty() {
        return Ok(());
    }
    uuid::Uuid::parse_str(request_id)
        .map(|_| ())
        .map_err(|_| Error::NotAUuid {
            len: request_id.len(),
        })
}

/// The three outcomes of looking a `request_id` up against what the server has
/// already seen, per AIP-155. Naming the contract is this crate's job; storing
/// the seen ids and their responses is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// The id is unseen: perform the operation, then record the request and its
    /// response under the id.
    New,
    /// The id was seen with the *same* request: return the recorded response
    /// instead of acting again.
    Replayed,
    /// The id was seen with a *different* request: reject it as a reused id. See
    /// [`conflict`] for the AIP-193 rejection `Status`.
    Conflict,
}

impl Replay {
    /// Decide the outcome from the server's record for the id: `None` when the
    /// id is unseen, `Some(true)` when the recorded request is identical to the
    /// incoming one, and `Some(false)` when it differs.
    pub fn decide(recorded_matches: Option<bool>) -> Self {
        match recorded_matches {
            None => Replay::New,
            Some(true) => Replay::Replayed,
            Some(false) => Replay::Conflict,
        }
    }
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying `REQUEST_ID_INVALID` + the `aip-rs` [`domain`](ERROR_DOMAIN) and
    /// the id length as `metadata`. A `request_id` is an opaque value rather than
    /// a named request-field path, so no `BadRequest` is attached (ADR-0007).
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let Error::NotAUuid { len } = err;
        let metadata = HashMap::from([("length".to_owned(), len.to_string())]);
        let mut details = ErrorDetails::new();
        details.set_error_info("REQUEST_ID_INVALID", ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

/// The AIP-193 rejection for a [`Replay::Conflict`] — a `request_id` replayed
/// with a different request body.
///
/// Maps to `ALREADY_EXISTS` (a reused idempotency token collides with the first
/// request's result; HTTP 409) with an `ErrorInfo` carrying `REQUEST_ID_CONFLICT`,
/// the `aip-rs` [`domain`](ERROR_DOMAIN), and the offending `request_id` as
/// `metadata`. AIP-155 does not fix a code for this case; `ALREADY_EXISTS` is
/// chosen for the collision semantics over a flat `INVALID_ARGUMENT`.
#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
pub fn conflict(request_id: &str) -> tonic::Status {
    use std::collections::HashMap;
    use tonic_types::{ErrorDetails, StatusExt};

    let metadata = HashMap::from([("request_id".to_owned(), request_id.to_owned())]);
    let mut details = ErrorDetails::new();
    details.set_error_info("REQUEST_ID_CONFLICT", ERROR_DOMAIN, metadata);
    tonic::Status::with_error_details(
        tonic::Code::AlreadyExists,
        format!("request_id `{request_id}` was already used for a different request"),
        details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_empty_as_no_idempotency() {
        assert_eq!(validate(""), Ok(()));
    }

    #[test]
    fn validate_accepts_any_uuid() {
        // UUIDv4 (what the field advertises) and a non-v4 UUID both parse.
        assert_eq!(validate("49351204-7395-47f1-9681-d48044b48c71"), Ok(()));
        assert_eq!(validate("00000000-0000-0000-0000-000000000000"), Ok(()));
    }

    #[test]
    fn validate_rejects_non_uuid() {
        assert_eq!(validate("not-a-uuid"), Err(Error::NotAUuid { len: 10 }));
        // A resource-id-shaped value is still not a UUID.
        assert_eq!(validate("abcd-efgh-1234"), Err(Error::NotAUuid { len: 14 }));
    }

    #[test]
    fn decide_maps_record_to_outcome() {
        assert_eq!(Replay::decide(None), Replay::New);
        assert_eq!(Replay::decide(Some(true)), Replay::Replayed);
        assert_eq!(Replay::decide(Some(false)), Replay::Conflict);
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn invalid_id_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::NotAUuid { len: 10 }.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "REQUEST_ID_INVALID");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.metadata.get("length").map(String::as_str), Some("10"));

        // A request_id is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn conflict_maps_to_already_exists_with_request_id() {
        let status = conflict("49351204-7395-47f1-9681-d48044b48c71");
        assert_eq!(status.code(), tonic::Code::AlreadyExists);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "REQUEST_ID_CONFLICT");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("request_id").map(String::as_str),
            Some("49351204-7395-47f1-9681-d48044b48c71"),
        );
    }
}
