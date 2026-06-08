//! `google.iam.v1` primitives: parse and validate the IAM identity vocabulary.
//!
//! The parsing core is pure string work with no protobuf dependency — a
//! [`Member`], a [`Role`], and a [`Permission`] each parse from and render back to
//! their `google.iam.v1` text form (`FromStr` / [`Display`](std::fmt::Display)).
//! Like the rest of aip-rs this layer *parses and validates*; the authorization
//! **decision** (role→permission expansion and IAM condition evaluation) is left
//! to the caller. The `google.iam.v1.Policy` structure, the AIP-211
//! `PERMISSION_DENIED` error shape, and the condition→CEL bridge land on top of
//! this core — see [`docs/adr/0010-iam-primitives.md`].
//!
//! See <https://google.aip.dev/211> (authorization checks) and
//! <https://google.aip.dev/213> (which blesses `google.iam.v1` for re-use).

mod member;
mod permission;
mod role;

pub use member::Member;
pub use permission::Permission;
pub use role::Role;

/// Errors produced parsing the IAM identity vocabulary.
///
/// One error type per crate (ADR-0001); each variant carries the dynamic values
/// its AIP-193 mapping reports as `metadata` (ADR-0007).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// A [`Member`] string was empty.
    #[error("member must not be empty")]
    MemberEmpty,
    /// A [`Member`] used a `type:` prefix this crate does not model.
    #[error(
        "unknown member type {prefix:?} (expected user:, serviceAccount:, group:, \
         domain:, allUsers, or allAuthenticatedUsers)"
    )]
    MemberUnknownType {
        /// The unrecognised prefix (or the whole string if it had no `:`).
        prefix: String,
    },
    /// A typed [`Member`] (`user:`, `group:`, …) had nothing after the `:`.
    #[error("member type {kind:?} must have a non-empty value after ':'")]
    MemberEmptyValue {
        /// The member type whose value was missing.
        kind: String,
    },
    /// A [`Role`] matched none of the recognised name forms.
    #[error(
        "role {role:?} must be roles/{{role}}, projects/{{p}}/roles/{{r}}, or \
         organizations/{{o}}/roles/{{r}}"
    )]
    RoleMalformed {
        /// The offending role name.
        role: String,
    },
    /// A [`Permission`] was not of the form `service.resource.verb`.
    #[error("permission {permission:?} must be of the form service.resource.verb")]
    PermissionMalformed {
        /// The offending permission name.
        permission: String,
    },
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain. See `docs/adr/0007-aip193-error-details.md`.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `IAM_*` `reason` + [`domain`](ERROR_DOMAIN) and
    /// the error's dynamic values as `metadata`. These are *validation* errors; the
    /// AIP-211 `PERMISSION_DENIED` authorization-failure shape is a separate helper
    /// (see `docs/adr/0010-iam-primitives.md`). A member/role/permission is an
    /// opaque value, not a request field path, so no `BadRequest` is attached.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::MemberEmpty => ("IAM_MEMBER_EMPTY", HashMap::new()),
            Error::MemberUnknownType { prefix } => (
                "IAM_MEMBER_UNKNOWN_TYPE",
                HashMap::from([("prefix".to_owned(), prefix.clone())]),
            ),
            Error::MemberEmptyValue { kind } => (
                "IAM_MEMBER_EMPTY_VALUE",
                HashMap::from([("kind".to_owned(), kind.clone())]),
            ),
            Error::RoleMalformed { role } => (
                "IAM_ROLE_MALFORMED",
                HashMap::from([("role".to_owned(), role.clone())]),
            ),
            Error::PermissionMalformed { permission } => (
                "IAM_PERMISSION_MALFORMED",
                HashMap::from([("permission".to_owned(), permission.clone())]),
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
    fn unknown_member_type_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::MemberUnknownType {
            prefix: "robot".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "IAM_MEMBER_UNKNOWN_TYPE");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.metadata.get("prefix").map(String::as_str), Some("robot"));

        // A member is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }
}
