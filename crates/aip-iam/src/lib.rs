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

/// Structural read-modify-write ops over a [`google.iam.v1.Policy`](proto::Policy)
/// — opt-in via the non-default `iam-proto` feature (ADR-0010). Binding
/// add/remove, dedupe/normalise, the `etag` optimistic-concurrency cycle, and the
/// *conditions ⟹ version 3* invariant.
#[cfg(feature = "iam-proto")]
pub mod policy;

/// The generated `google.iam.v1` Policy structure — opt-in via the non-default
/// `iam-proto` feature (ADR-0010).
///
/// The vendored `policy.proto` is compiled with `protox` (no `protoc`, ADR-0001)
/// and `prost-build`, mirroring `aip-filtering`'s `cel-proto`. This is the
/// structural layer the read-modify-write ops (binding add/remove, dedupe, the
/// `etag` cycle) build on; the parse/validate core above stays proto-free, so a
/// default build pulls in no proto runtime.
///
/// [`Policy`] / [`Binding`] are re-exported for convenience; the remaining
/// `policy.proto` messages (audit config, deltas) and the `google.type.Expr`
/// condition live under [`google`](proto::google).
#[cfg(feature = "iam-proto")]
pub mod proto {
    #![allow(missing_docs, clippy::all, rustdoc::all)]

    /// The generated `google.*` protobuf packages, mounted in a module tree that
    /// mirrors each package path so prost's cross-package reference from
    /// `Binding.condition` to `google.type.Expr` resolves.
    pub mod google {
        pub mod iam {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/google.iam.v1.rs"));
            }
        }
        pub mod r#type {
            // prost escapes the `type` keyword in the generated file name, too.
            include!(concat!(env!("OUT_DIR"), "/google.r#type.rs"));
        }
    }

    pub use google::iam::v1::{Binding, Policy};
}

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
    /// A [`Policy`](proto::Policy) carried a **Binding** with a **Condition** but
    /// its schema `version` was not `3`. IAM requires policy version `3` for any
    /// conditional binding (the *conditions ⟹ version 3* invariant; ADR-0010).
    #[error(
        "a conditional binding requires policy version 3, but the policy is version {version}"
    )]
    PolicyConditionRequiresVersion3 {
        /// The policy's `version` field as supplied.
        version: i32,
    },
    /// The `etag` supplied to a read-modify-write [`SetIamPolicy`] did not match
    /// the [`Policy`](proto::Policy) currently stored — a concurrent modification
    /// intervened. The caller must re-read the policy and retry (ADR-0010).
    ///
    /// [`SetIamPolicy`]: https://google.aip.dev/211
    #[error("policy etag mismatch: the policy was modified concurrently; re-read and retry")]
    PolicyEtagMismatch,
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain. See `docs/adr/0007-aip193-error-details.md`.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to a canonical gRPC code with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `IAM_*` `reason` + [`domain`](ERROR_DOMAIN) and
    /// the error's dynamic values as `metadata`.
    ///
    /// Most are *validation* errors and map to `INVALID_ARGUMENT`; the
    /// read-modify-write [`PolicyEtagMismatch`](Error::PolicyEtagMismatch) maps to
    /// `ABORTED`, matching the IAM optimistic-concurrency contract — a stale `etag`
    /// is a concurrency conflict, not a malformed request (ADR-0010). The AIP-211
    /// `PERMISSION_DENIED` authorization-failure shape is a separate helper. A
    /// member/role/permission is an opaque value, not a request field path, so no
    /// `BadRequest` is attached.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (code, reason, metadata): (tonic::Code, &str, HashMap<String, String>) = match &err {
            Error::MemberEmpty => (
                tonic::Code::InvalidArgument,
                "IAM_MEMBER_EMPTY",
                HashMap::new(),
            ),
            Error::MemberUnknownType { prefix } => (
                tonic::Code::InvalidArgument,
                "IAM_MEMBER_UNKNOWN_TYPE",
                HashMap::from([("prefix".to_owned(), prefix.clone())]),
            ),
            Error::MemberEmptyValue { kind } => (
                tonic::Code::InvalidArgument,
                "IAM_MEMBER_EMPTY_VALUE",
                HashMap::from([("kind".to_owned(), kind.clone())]),
            ),
            Error::RoleMalformed { role } => (
                tonic::Code::InvalidArgument,
                "IAM_ROLE_MALFORMED",
                HashMap::from([("role".to_owned(), role.clone())]),
            ),
            Error::PermissionMalformed { permission } => (
                tonic::Code::InvalidArgument,
                "IAM_PERMISSION_MALFORMED",
                HashMap::from([("permission".to_owned(), permission.clone())]),
            ),
            Error::PolicyConditionRequiresVersion3 { version } => (
                tonic::Code::InvalidArgument,
                "IAM_POLICY_CONDITION_REQUIRES_VERSION_3",
                HashMap::from([("version".to_owned(), version.to_string())]),
            ),
            Error::PolicyEtagMismatch => (
                tonic::Code::Aborted,
                "IAM_POLICY_ETAG_MISMATCH",
                HashMap::new(),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(code, message, details)
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
        assert_eq!(
            info.metadata.get("prefix").map(String::as_str),
            Some("robot")
        );

        // A member is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn etag_mismatch_maps_to_aborted() {
        // A stale etag is an optimistic-concurrency conflict, not a malformed
        // request, so it maps to ABORTED (the IAM read-modify-write contract) —
        // unlike every other (validation) error, which is INVALID_ARGUMENT.
        let status: tonic::Status = Error::PolicyEtagMismatch.into();
        assert_eq!(status.code(), tonic::Code::Aborted);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "IAM_POLICY_ETAG_MISMATCH");
        assert_eq!(info.domain, ERROR_DOMAIN);
    }
}

#[cfg(all(test, feature = "iam-proto"))]
mod proto_tests {
    use super::proto::{Binding, Policy};
    use prost::Message as _;

    /// The `iam-proto` feature really does generate a usable `google.iam.v1`
    /// Policy: a conditional version-3 policy survives an encode/decode round-trip
    /// unchanged, proving the vendored `policy.proto` (and the `google.type.Expr`
    /// condition it imports) compiled into prost types correctly.
    #[test]
    fn policy_round_trips_through_the_wire() {
        let policy = Policy {
            version: 3,
            bindings: vec![Binding {
                role: "roles/viewer".to_owned(),
                members: vec![
                    "user:alice@example.com".to_owned(),
                    "group:admins@example.com".to_owned(),
                ],
                condition: Some(super::proto::google::r#type::Expr {
                    expression: "request.time < timestamp(\"2030-01-01T00:00:00Z\")".to_owned(),
                    title: "expirable access".to_owned(),
                    ..Default::default()
                }),
            }],
            etag: b"BwWWja0YfJA=".to_vec(),
            audit_configs: Vec::new(),
        };

        let decoded = Policy::decode(policy.encode_to_vec().as_slice()).expect("decode");
        assert_eq!(decoded, policy);
    }
}
