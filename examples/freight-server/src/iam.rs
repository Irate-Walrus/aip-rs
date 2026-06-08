//! The `google.iam.v1.IAMPolicy` service — the IAM tracer bullet (aip #64).
//!
//! Proves a **Member** string round-trips through the IAM standard methods, the
//! way #39 drove a **Filter** into SQLite. `SetIamPolicy` validates every Member
//! of every **Binding** via `aip::iam`, stores the **Policy** keyed by **Resource
//! name**, and `GetIamPolicy` returns it (an empty Policy when none is set). So a
//! Member travels request → validate → store → response.
//!
//! `TestIamPermissions` is left as a `TODO(aip #68)` seam: it decides the held
//! permission subset *through* the opt-in cel-backed `eval` adapter (#66), which
//! lands in a later slice (ADR-0010).

use tonic::{Request, Response, Status};

use crate::proto::google::iam::v1::{
    iam_policy_server::IamPolicy, GetIamPolicyRequest, Policy, SetIamPolicyRequest,
    TestIamPermissionsRequest, TestIamPermissionsResponse,
};
use crate::storage::PolicyStore;

/// Serves `IAMPolicy` over an in-memory, resource-name-keyed [`PolicyStore`].
#[derive(Default)]
pub struct IamServer {
    policies: PolicyStore,
}

impl IamServer {
    /// A server backed by an empty policy store.
    pub fn new() -> Self {
        Self {
            policies: PolicyStore::new(),
        }
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

        // Replace any existing policy (IAM `SetIamPolicy` semantics), then echo
        // the stored policy back so the round-trip is observable in one call.
        self.policies.set(req.resource, policy.clone());
        Ok(Response::new(policy))
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
    use crate::proto::google::iam::v1::Binding;

    /// A freight resource name a policy attaches to.
    const RESOURCE: &str = "shippers/acme";

    /// A policy granting `role` to `members`.
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

    #[tokio::test]
    async fn set_then_get_round_trips_a_well_formed_policy() {
        // A Member travels request → validate → store → response: a well-formed
        // policy is accepted by `SetIamPolicy` and read back identically by
        // `GetIamPolicy`.
        let server = IamServer::new();
        let stored = policy(
            "roles/viewer",
            &["user:alice@example.com", "group:ops@example.com"],
        );

        let set = server
            .set_iam_policy(Request::new(SetIamPolicyRequest {
                resource: RESOURCE.to_owned(),
                policy: Some(stored.clone()),
                update_mask: None,
            }))
            .await
            .expect("a well-formed policy is accepted")
            .into_inner();
        assert_eq!(set, stored);

        let got = server
            .get_iam_policy(Request::new(GetIamPolicyRequest {
                resource: RESOURCE.to_owned(),
                options: None,
            }))
            .await
            .expect("get succeeds")
            .into_inner();
        assert_eq!(got, stored);
    }

    #[tokio::test]
    async fn get_on_unset_resource_returns_empty_policy() {
        // `GetIamPolicy` on a resource with no policy is not an error — it returns
        // the empty `Policy`.
        let server = IamServer::new();
        let got = server
            .get_iam_policy(Request::new(GetIamPolicyRequest {
                resource: "shippers/never-set".to_owned(),
                options: None,
            }))
            .await
            .expect("an unset policy is not an error")
            .into_inner();
        assert_eq!(got, Policy::default());
    }

    #[tokio::test]
    async fn set_rejects_a_malformed_member_with_iam_error_info() {
        use tonic_types::StatusExt as _;

        // A malformed Member is rejected via aip-iam's AIP-193 mapping:
        // `INVALID_ARGUMENT` carrying an `IAM_*` `ErrorInfo` under the `aip-rs`
        // domain. The policy never reaches the store.
        let server = IamServer::new();
        let status = server
            .set_iam_policy(Request::new(SetIamPolicyRequest {
                resource: RESOURCE.to_owned(),
                policy: Some(policy("roles/viewer", &["robot:r2d2"])),
                update_mask: None,
            }))
            .await
            .expect_err("a malformed member is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "IAM_MEMBER_UNKNOWN_TYPE");
        assert_eq!(info.domain, "aip-rs");

        // The rejected policy was not stored.
        assert_eq!(
            server
                .get_iam_policy(Request::new(GetIamPolicyRequest {
                    resource: RESOURCE.to_owned(),
                    options: None,
                }))
                .await
                .expect("get succeeds")
                .into_inner(),
            Policy::default(),
        );
    }
}
