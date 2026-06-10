//! AIP-154 resource freshness validation: content etags for optimistic
//! concurrency, over any resource.
//!
//! This generalizes the `etag` cycle `aip-iam` built for the IAM **Policy**
//! ([`compute_etag`](aip_iam::policy::compute_etag) /
//! [`check_etag`](aip_iam::policy::check_etag)) into a primitive any resource can
//! use. [`compute_etag`] derives a deterministic content digest of a resource
//! that a server returns to clients; [`check_etag`] verifies the etag a client
//! sends back on update/delete before acting, so a concurrent writer can no
//! longer silently clobber.
//!
//! # The digest scheme
//!
//! The etag is a CRC32-IEEE digest of the resource, rendered as eight lowercase
//! hex digits — the same content-digest idiom `aip-iam`'s Policy etag and
//! `aip-pagination`'s request checksum use. Two fields are excluded before
//! hashing:
//!
//! - **the `etag` field itself** — it is not part of the content it summarises
//!   (an empty and a stamped copy of the same resource must share a token); and
//! - **every `OUTPUT_ONLY` field** (server-owned timestamps, computed fields),
//!   so the token digests the resource's *content*, not its server-side churn.
//!   The exclusion is read off `google.api.field_behavior` via
//!   [`aip_fieldbehavior::clear_fields_dynamic`].
//!
//! Excluding `OUTPUT_ONLY` makes this a **weak** validator in RFC 7232 terms:
//! two resources bearing the same etag are equivalent but may differ in
//! server-owned fields. The token is opaque — compared only for equality, never
//! parsed — so it deliberately omits the RFC 7232 surface form AIP-154 layers on
//! for HTTP (the `W/` weak-validator prefix, and the surrounding quotes AIP-154
//! recommends), staying byte-compatible with the IAM Policy scheme instead. For a
//! resource that has no `OUTPUT_ONLY` fields (e.g. `google.iam.v1.Policy`) the two
//! schemes produce identical tokens.
//!
//! The digest is deterministic for any resource whose protobuf encoding is
//! deterministic. A resource carrying an unordered map should be normalised
//! before hashing (as `aip-iam` normalises a Policy's bindings) so that
//! semantically-equal values compare equal — the same caveat
//! [`aip_pagination::request_checksum`] carries.
//!
//! # Reflection
//!
//! This is a **Reflective primitive** (ADR-0009): it needs a message's
//! **Descriptor** to find the `etag` field and the `OUTPUT_ONLY` annotations.
//! Like [`aip_pagination::request_checksum`] — and unlike the field-mask
//! primitive — it is a pair of *single generic functions* over
//! [`ReflectMessage`], not a Typed-facade / Dynamic-core split: both only read
//! the resource (no decode-back, so no `Default` bound), and because
//! [`DynamicMessage`] itself implements [`ReflectMessage`] a caller holding a
//! dynamic message (JSON ingestion, a generic gateway) calls them directly.
//!
//! See <https://google.aip.dev/154>.

use prost::Message as _;
use prost_reflect::{DynamicMessage, ReflectMessage};

use aip_fieldbehavior::{clear_fields_dynamic, FieldBehavior};

/// The name of the resource's checksum field, excluded from the digest. Per
/// AIP-154 the field **must** be named `etag`.
const ETAG_FIELD: &str = "etag";

/// Errors produced verifying a supplied etag against the stored resource.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// The supplied etag is well-formed but does not match the current resource —
    /// a concurrent modification intervened between the client's read and this
    /// write. The caller must re-read the resource and retry. AIP-154 maps this
    /// to `ABORTED`.
    #[error("etag mismatch: the resource was modified concurrently; re-read and retry")]
    Mismatch,
    /// The supplied etag could not have been minted by [`compute_etag`] — it is
    /// not eight lowercase hex digits. A malformed *request value*, distinct from
    /// a stale one, so it maps to `INVALID_ARGUMENT` rather than `ABORTED`.
    #[error("malformed etag {etag:?}: expected the opaque token a prior read returned")]
    Malformed {
        /// The offending etag as supplied.
        etag: String,
    },
}

/// The content `etag` of `resource`: a deterministic digest used as the
/// optimistic-concurrency token for the read-modify-write cycle.
///
/// The `etag` field and every `OUTPUT_ONLY` field are excluded before hashing,
/// so the value is a pure function of the resource's content — recomputing it
/// over a stored resource reproduces the token a prior read returned. The result
/// is eight lowercase hex digits; see the [crate docs](self) for the full digest
/// scheme and its determinism caveat.
pub fn compute_etag<M: ReflectMessage>(resource: &M) -> String {
    // Transcode through wire bytes into a dynamic message so the etag and
    // OUTPUT_ONLY fields can be cleared by reflection. The round-trip can only
    // fail if a message and its descriptor disagree — a build/config bug, not
    // bad input — so it is an invariant, not an error variant (ADR-0009).
    let mut dynamic =
        DynamicMessage::decode(resource.descriptor(), resource.encode_to_vec().as_slice())
            .expect("a message round-trips through its own descriptor");
    // Exclude server-owned noise so the token digests content, not churn.
    clear_fields_dynamic(&mut dynamic, &[FieldBehavior::OutputOnly]);
    // Exclude the etag field itself (it is not part of the content it summarises).
    if let Some(field) = dynamic.descriptor().get_field_by_name(ETAG_FIELD) {
        dynamic.clear_field(&field);
    }
    let digest = crc32fast::hash(&dynamic.encode_to_vec());
    format!("{digest:08x}")
}

/// Optimistic-concurrency check for an update/delete: decide whether a write
/// carrying `supplied` (the etag the client sent back) may proceed against
/// `current` (the resource presently stored).
///
/// - An **empty** `supplied` is an unconditional write — the client opted out of
///   the freshness check, so it always proceeds (AIP-154).
/// - A `supplied` that is not the eight-lowercase-hex form [`compute_etag`] mints
///   could never have come from a prior read, so it is [`Error::Malformed`].
/// - Otherwise `supplied` must equal [`compute_etag`] of `current`; a mismatch
///   means another writer intervened ([`Error::Mismatch`]).
///
/// The caller resolves a missing resource (`NOT_FOUND`) before reaching this
/// check, so a resource is always in hand — unlike `aip-iam`'s policy check,
/// whose stored policy may be absent.
///
/// # Errors
///
/// [`Error::Mismatch`] for a stale etag (AIP-154 maps it to `ABORTED`);
/// [`Error::Malformed`] for one that is not a well-formed token (mapped to
/// `INVALID_ARGUMENT`).
pub fn check_etag<M: ReflectMessage>(supplied: &str, current: &M) -> Result<(), Error> {
    if supplied.is_empty() {
        return Ok(());
    }
    if !is_well_formed(supplied) {
        return Err(Error::Malformed {
            etag: supplied.to_owned(),
        });
    }
    if supplied == compute_etag(current) {
        Ok(())
    } else {
        Err(Error::Mismatch)
    }
}

/// Whether `etag` has the exact shape [`compute_etag`] mints: eight lowercase
/// hex digits. Anything else could not be a token this crate issued.
fn is_well_formed(etag: &str) -> bool {
    etag.len() == 8 && etag.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain. See `docs/adr/0007-aip193-error-details.md`.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to a canonical gRPC code with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `ETAG_*` `reason` + [`domain`](ERROR_DOMAIN)
    /// and the error's dynamic values as `metadata`.
    ///
    /// A stale [`Mismatch`](Error::Mismatch) maps to `ABORTED` — the AIP-154
    /// optimistic-concurrency contract: a concurrent modification, telling the
    /// caller to re-read and retry. A [`Malformed`](Error::Malformed) etag maps to
    /// `INVALID_ARGUMENT`: it is a bad request value, not a concurrency conflict.
    /// An etag is an opaque value rather than a named request field path, so no
    /// `BadRequest` is attached (matching `aip-iam`'s policy etag).
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (code, reason, metadata): (tonic::Code, &str, HashMap<String, String>) = match &err {
            Error::Mismatch => (tonic::Code::Aborted, "ETAG_MISMATCH", HashMap::new()),
            Error::Malformed { etag } => (
                tonic::Code::InvalidArgument,
                "ETAG_MALFORMED",
                HashMap::from([("etag".to_owned(), etag.clone())]),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(code, message, details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_reflect::DynamicMessage;

    /// Builds a `Shipper` fixture (it carries the `name`/`display_name` content
    /// fields, the `OUTPUT_ONLY` timestamps, and the `etag` field) from JSON.
    fn shipper(json: &str) -> DynamicMessage {
        test_fixtures::from_json("einride.example.freight.v1.Shipper", json)
            .expect("Shipper fixture builds")
    }

    #[test]
    fn etag_is_stable_and_ignores_the_etag_field() {
        let etag = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#));

        // Recomputing over equal content reproduces the token.
        assert_eq!(
            compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#)),
            etag,
        );
        // The same content carrying a different etag yields the same token.
        let stamped = shipper(r#"{"name":"shippers/acme","displayName":"Acme","etag":"deadbeef"}"#);
        assert_eq!(compute_etag(&stamped), etag);
    }

    #[test]
    fn etag_ignores_output_only_fields() {
        let etag = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#));

        // create_time / update_time are OUTPUT_ONLY: server-stamped churn must not
        // move the content etag.
        let stamped = shipper(
            r#"{"name":"shippers/acme","displayName":"Acme",
                "createTime":"2024-01-01T00:00:00Z","updateTime":"2024-02-02T00:00:00Z"}"#,
        );
        assert_eq!(compute_etag(&stamped), etag);
    }

    #[test]
    fn etag_changes_when_content_changes() {
        let a = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#));
        let b = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Beta"}"#));
        assert_ne!(a, b, "a content change must flip the token");
    }

    #[test]
    fn etag_is_eight_lowercase_hex() {
        let etag = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#));
        assert!(is_well_formed(&etag), "minted etag {etag:?} is well-formed");
    }

    #[test]
    fn check_allows_an_empty_supplied_etag() {
        let current = shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#);
        // An unconditional write opts out of the freshness check (AIP-154).
        assert_eq!(check_etag("", &current), Ok(()));
    }

    #[test]
    fn check_accepts_a_matching_etag_and_rejects_a_stale_one() {
        let current = shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#);
        let fresh = compute_etag(&current);
        assert_eq!(check_etag(&fresh, &current), Ok(()));

        // A token minted before a concurrent edit no longer matches the current
        // content — a stale read, ABORTED on the wire.
        let stale = compute_etag(&shipper(r#"{"name":"shippers/acme","displayName":"Old"}"#));
        assert_eq!(check_etag(&stale, &current), Err(Error::Mismatch));
    }

    #[test]
    fn check_rejects_a_malformed_etag() {
        let current = shipper(r#"{"name":"shippers/acme","displayName":"Acme"}"#);
        // Too short, non-hex, too long, and the wrong case all fail the format
        // check before any comparison — a value no prior read could have issued.
        for bad in ["abc", "not-hex!", "deadbeef0", "DEADBEEF"] {
            assert_eq!(
                check_etag(bad, &current),
                Err(Error::Malformed {
                    etag: bad.to_owned()
                }),
                "{bad:?} should be malformed",
            );
        }
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn mismatch_maps_to_aborted() {
        // A stale etag is an optimistic-concurrency conflict, not a malformed
        // request, so it maps to ABORTED (the AIP-154 contract).
        let status: tonic::Status = Error::Mismatch.into();
        assert_eq!(status.code(), tonic::Code::Aborted);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "ETAG_MISMATCH");
        assert_eq!(info.domain, ERROR_DOMAIN);

        // An etag is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn malformed_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::Malformed {
            etag: "nope".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "ETAG_MALFORMED");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.metadata.get("etag").map(String::as_str), Some("nope"));

        assert!(status.get_details_bad_request().is_none());
    }
}
