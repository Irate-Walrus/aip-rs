//! The `google.iam.v1.IAMPolicy` service — driving the `aip::iam` Policy toolkit.
//!
//! Started as the IAM tracer bullet (aip #64): a **Member** string round-trips
//! through the standard methods, the way #39 drove a **Filter** into SQLite. With
//! the structural read-modify-write ops in place (aip #65), `SetIamPolicy` now
//! mutates policies *through* the helpers rather than blind-overwriting:
//!
//! 1. every **Member** of every **Binding** is parsed via `aip::iam` (validate);
//! 2. the *conditions ⟹ version 3* invariant is enforced
//!    ([`aip::iam::policy::validate`]);
//! 3. the supplied `etag` is checked against the stored policy — a stale token is
//!    rejected with `ABORTED` ([`aip::iam::policy::check_etag`]) — then the policy
//!    is normalised, re-stamped with a fresh `etag`, and stored atomically
//!    ([`PolicyStore::set_checked`]).
//!
//! `GetIamPolicy` returns the stored policy (carrying its `etag`), or an empty
//! Policy when none is set. The `Policy` / `Binding` types are the very ones the
//! helpers operate on — shared from `aip::iam::proto` via `extern_path` (aip #65).
//!
//! `TestIamPermissions` is left as a `TODO(aip #68)` seam: it decides the held
//! permission subset *through* the opt-in cel-backed `eval` adapter (#66), which
//! lands in a later slice (ADR-0010).

use std::sync::Arc;

use tonic::{Request, Response, Status};

// The `IAMPolicy` service trait and its request/response messages are generated
// locally; the `Policy` / `Binding` message layer is shared with the structural
// helpers via `extern_path` (aip #65), so it comes from `aip::iam::proto`.
use crate::proto::google::iam::v1::{
    iam_policy_server::IamPolicy, GetIamPolicyRequest, SetIamPolicyRequest,
    TestIamPermissionsRequest, TestIamPermissionsResponse,
};
use crate::storage::PolicyStore;
use aip::iam::proto::Policy;

/// Serves `IAMPolicy` over an in-memory, resource-name-keyed [`PolicyStore`].
///
/// The store is shared (`Arc`) with [`FreightService`](crate::service::FreightServer)
/// so a Policy set here governs that service's AIP-211 authorization (aip #67).
#[derive(Default)]
pub struct IamServer {
    policies: Arc<PolicyStore>,
}

impl IamServer {
    /// A server backed by its own empty policy store. The binary always shares a
    /// store via [`with_store`](Self::with_store), so this stand-alone constructor
    /// is only used by the service tests.
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            policies: Arc::new(PolicyStore::new()),
        }
    }

    /// A server backed by an existing, shared policy store — the one
    /// [`FreightService`](crate::service::FreightServer) reads to authorize.
    pub fn with_store(policies: Arc<PolicyStore>) -> Self {
        Self { policies }
    }
}

#[tonic::async_trait]
impl IamPolicy for IamServer {
    async fn set_iam_policy(
        &self,
        request: Request<SetIamPolicyRequest>,
    ) -> Result<Response<Policy>, Status> {
        let req = request.into_inner();
        if req.resource.is_empty() {
            return Err(Status::invalid_argument("resource is required"));
        }
        let policy = req
            .policy
            .ok_or_else(|| Status::invalid_argument("policy is required"))?;

        // Validate every Member of every Binding via aip-iam. A malformed member
        // converts through the crate's AIP-193 `From<Error> for Status` (#16) to
        // `INVALID_ARGUMENT` carrying an `IAM_*` `ErrorInfo` — the validate step
        // the Member passes before it ever reaches the store.
        for binding in &policy.bindings {
            for member in &binding.members {
                member.parse::<aip::iam::Member>()?;
            }
        }

        // Enforce the conditions ⟹ version-3 invariant (INVALID_ARGUMENT via the
        // same AIP-193 mapping) before the policy is stored.
        aip::iam::policy::validate(&policy)?;

        // Read-modify-write through the helpers: a stale `etag` is rejected with
        // `ABORTED`, the accepted policy is normalised and re-stamped with a fresh
        // `etag`, and stored atomically. Echo the stored policy so the new `etag`
        // is observable in one call.
        let stored = self.policies.set_checked(req.resource, policy)?;
        Ok(Response::new(stored))
    }

    async fn get_iam_policy(
        &self,
        request: Request<GetIamPolicyRequest>,
    ) -> Result<Response<Policy>, Status> {
        let resource = request.into_inner().resource;
        if resource.is_empty() {
            return Err(Status::invalid_argument("resource is required"));
        }
        // A resource with no policy set returns the empty `Policy`, not an error
        // (the IAM `GetIamPolicy` contract).
        Ok(Response::new(
            self.policies.get(&resource).unwrap_or_default(),
        ))
    }

    async fn test_iam_permissions(
        &self,
        _: Request<TestIamPermissionsRequest>,
    ) -> Result<Response<TestIamPermissionsResponse>, Status> {
        // TODO(aip #68): decide the held subset of the requested permissions
        // through the opt-in cel-backed `eval` adapter (#66) over the stored
        // Policy and an example-owned role→permission catalogue.
        Err(Status::unimplemented(
            "TestIamPermissions is not implemented yet in the aip-rs demo (aip #68)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aip::iam::proto::google::r#type::Expr;
    use aip::iam::proto::Binding;

    /// A freight resource name a policy attaches to.
    const RESOURCE: &str = "shippers/acme";

    /// A version-1 policy granting `role` to `members`, with no `etag`.
    fn policy(role: &str, members: &[&str]) -> Policy {
        Policy {
            version: 1,
            bindings: vec![Binding {
                role: role.to_owned(),
                members: members.iter().map(|m| (*m).to_owned()).collect(),
                condition: None,
            }],
            etag: Vec::new(),
            audit_configs: Vec::new(),
        }
    }

    /// Drive `SetIamPolicy`, returning the stored policy (or the `Status`).
    async fn set(server: &IamServer, policy: Policy) -> Result<Policy, Status> {
        server
            .set_iam_policy(Request::new(SetIamPolicyRequest {
                resource: RESOURCE.to_owned(),
                policy: Some(policy),
                update_mask: None,
            }))
            .await
            .map(Response::into_inner)
    }

    /// Drive `GetIamPolicy` for `resource`.
    async fn get(server: &IamServer, resource: &str) -> Policy {
        server
            .get_iam_policy(Request::new(GetIamPolicyRequest {
                resource: resource.to_owned(),
                options: None,
            }))
            .await
            .expect("get succeeds")
            .into_inner()
    }

    #[tokio::test]
    async fn set_normalises_and_stamps_an_etag_then_get_round_trips() {
        // A Member travels request → validate → store → response. The policy is
        // normalised (members in canonical order) and stamped with a fresh `etag`
        // before storage, and `GetIamPolicy` reads back exactly what was stored.
        let server = IamServer::new();
        let stored = set(
            &server,
            policy(
                "roles/viewer",
                &["user:alice@example.com", "group:ops@example.com"],
            ),
        )
        .await
        .expect("a well-formed policy is accepted");

        assert_eq!(
            stored.bindings,
            vec![Binding {
                role: "roles/viewer".to_owned(),
                // Sorted: "group:…" precedes "user:…".
                members: vec![
                    "group:ops@example.com".to_owned(),
                    "user:alice@example.com".to_owned(),
                ],
                condition: None,
            }],
            "members come back in canonical order",
        );
        assert!(!stored.etag.is_empty(), "the server stamps a fresh etag");

        assert_eq!(
            get(&server, RESOURCE).await,
            stored,
            "get round-trips the stored policy"
        );
    }

    #[tokio::test]
    async fn get_on_unset_resource_returns_empty_policy() {
        // `GetIamPolicy` on a resource with no policy is not an error — it returns
        // the empty `Policy`.
        let server = IamServer::new();
        assert_eq!(get(&server, "shippers/never-set").await, Policy::default());
    }

    #[tokio::test]
    async fn set_rejects_a_malformed_member_with_iam_error_info() {
        use tonic_types::StatusExt as _;

        // A malformed Member is rejected via aip-iam's AIP-193 mapping:
        // `INVALID_ARGUMENT` carrying an `IAM_*` `ErrorInfo` under the `aip-rs`
        // domain. The policy never reaches the store.
        let server = IamServer::new();
        let status = set(&server, policy("roles/viewer", &["robot:r2d2"]))
            .await
            .expect_err("a malformed member is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "IAM_MEMBER_UNKNOWN_TYPE");
        assert_eq!(info.domain, "aip-rs");

        // The rejected policy was not stored.
        assert_eq!(get(&server, RESOURCE).await, Policy::default());
    }

    #[tokio::test]
    async fn set_rejects_a_conditional_binding_below_version_3() {
        use tonic_types::StatusExt as _;

        // The conditions ⟹ version-3 invariant: a conditional binding on a
        // version-1 policy is `INVALID_ARGUMENT` with its own `IAM_*` reason.
        let server = IamServer::new();
        let mut conditional = policy("roles/viewer", &["user:alice@example.com"]);
        conditional.bindings[0].condition = Some(Expr {
            expression: "request.time < timestamp(\"2030-01-01T00:00:00Z\")".to_owned(),
            ..Expr::default()
        });

        let status = set(&server, conditional)
            .await
            .expect_err("a conditional version-1 policy is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(
            status
                .get_details_error_info()
                .expect("an ErrorInfo is attached")
                .reason,
            "IAM_POLICY_CONDITION_REQUIRES_VERSION_3",
        );
    }

    #[tokio::test]
    async fn set_rejects_a_stale_etag_with_aborted() {
        use tonic_types::StatusExt as _;

        // The read-modify-write contract: the first write (no etag, unconditional)
        // is accepted and stamped; a follow-up carrying that etag is accepted and
        // advances it; replaying the now-stale etag is rejected with `ABORTED`.
        let server = IamServer::new();
        let first = set(&server, policy("roles/viewer", &["user:alice@example.com"]))
            .await
            .expect("the first write is accepted");

        let mut second = policy(
            "roles/viewer",
            &["user:alice@example.com", "group:ops@example.com"],
        );
        second.etag = first.etag.clone();
        let second = set(&server, second)
            .await
            .expect("a fresh etag is accepted");
        assert_ne!(second.etag, first.etag, "the etag advances on each write");

        let mut stale = policy("roles/editor", &["user:bob@example.com"]);
        stale.etag = first.etag.clone();
        let status = set(&server, stale)
            .await
            .expect_err("replaying a stale etag is rejected");
        assert_eq!(status.code(), tonic::Code::Aborted);
        assert_eq!(
            status
                .get_details_error_info()
                .expect("an ErrorInfo is attached")
                .reason,
            "IAM_POLICY_ETAG_MISMATCH",
        );

        // The stale write did not take effect — the second write still stands.
        assert_eq!(get(&server, RESOURCE).await, second);
    }
}
