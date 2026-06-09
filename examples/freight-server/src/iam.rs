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
//! `TestIamPermissions` (aip #68) decides the held permission subset *through* the
//! opt-in cel-backed `eval` adapter (#66, ADR-0010): given the stored **Policy**,
//! it expands each **Binding**'s **Role** to **Permissions** via an example-owned
//! catalogue ([`role_permissions`] — `aip-iam` ships none), matches the caller's
//! **Member**, and evaluates any **Condition** against the request context. It
//! returns only the subset the caller holds and never errors on a permission the
//! caller simply lacks.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::SystemTime;

use tonic::{Request, Response, Status};

// The `IAMPolicy` service trait and its request/response messages are generated
// locally; the `Policy` / `Binding` message layer is shared with the structural
// helpers via `extern_path` (aip #65), so it comes from `aip::iam::proto`.
use crate::proto::google::iam::v1::{
    iam_policy_server::IamPolicy, GetIamPolicyRequest, SetIamPolicyRequest,
    TestIamPermissionsRequest, TestIamPermissionsResponse,
};
// `TestIamPermissions` reuses the freight service's caller-identity and
// member-matching helpers (#67/#68), so the two services decide membership the
// same way over the shared Policy store.
use crate::service::{caller_member, member_matches};
use crate::storage::PolicyStore;
use aip::iam::eval::{Condition, RequestContext};
use aip::iam::proto::Policy;
use aip::iam::{Member, Permission};

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

    /// The set of **Permissions** `caller` holds on `resource` per the stored
    /// **Policy** — the authorization *decision* `TestIamPermissions` returns a
    /// subset of (ADR-0010).
    ///
    /// It is the union, over every **Binding** `caller` is a **Member** of whose
    /// **Condition** holds, of that **Binding**'s **Role** expanded through the
    /// example-owned [`role_permissions`] catalogue. Three things the *caller* owns,
    /// not `aip-iam`: role→permission expansion (the library ships no role
    /// definitions), **Member** matching, and **Condition** evaluation through the
    /// opt-in cel-backed `eval` adapter (#66).
    ///
    /// A resource with **no Policy holds nothing**: the held subset is decided
    /// purely from the Policy's **Bindings**, *not* the read gate's "unprotected ⇒
    /// public" simplification ([`FreightServer::authorized`](crate::service)).
    ///
    /// # Errors
    ///
    /// A **Condition** that fails to compile or evaluate converts via aip-iam's
    /// AIP-193 `From<Error>` to `INVALID_ARGUMENT`: the eval adapter keeps a *broken*
    /// Condition distinct from one that simply did not hold (a `false` result just
    /// excludes the **Binding**'s permissions; ADR-0010).
    fn granted_permissions(
        &self,
        resource: &str,
        caller: Option<&Member>,
    ) -> Result<HashSet<Permission>, Status> {
        let Some(policy) = self.policies.get(resource) else {
            return Ok(HashSet::new());
        };

        // The IAM-style **Condition** environment the eval adapter exposes: the
        // resource under test (`resource.name`) and when the request arrived
        // (`request.time`). Built once and reused across the Policy's Bindings, so a
        // re-checked Condition compiles per evaluation but the context does not
        // rebuild.
        let context = RequestContext::new()
            .resource("name", resource)
            .request_time(SystemTime::now());

        let mut granted = HashSet::new();
        for binding in &policy.bindings {
            // Only a Binding the caller is a Member of can grant anything.
            if !binding.members.iter().any(|m| member_matches(m, caller)) {
                continue;
            }
            // A conditional Binding is gated on its Condition: compile it (general
            // CEL, not the AIP-160 subset) and evaluate against this request. A
            // `false` result excludes the Binding; a broken Condition is an error,
            // never a silent denial.
            if let Some(expr) = &binding.condition {
                if !Condition::compile(&expr.expression)?.evaluate(&context)? {
                    continue;
                }
            }
            // Expand the Binding's Role to its Permissions via the example-owned
            // catalogue. An unrecognised Role bundles nothing.
            granted.extend(
                role_permissions(&binding.role)
                    .into_iter()
                    .map(|permission| {
                        permission
                            .parse::<Permission>()
                            .expect("catalogue permissions are well-formed")
                    }),
            );
        }
        Ok(granted)
    }
}

/// The example-owned **Role**→**Permission** catalogue. `aip-iam` ships no role
/// definitions — role→permission expansion is the caller's responsibility
/// (ADR-0010) — so the demo owns this mapping from a freight **Role** to the
/// **Permissions** it bundles. `roles/freight.editor` is a superset of
/// `roles/freight.viewer` (it adds the write verbs), and `roles/freight.admin` a
/// superset of `editor` (it adds IAM-policy administration), the way a real role
/// hierarchy nests. An unrecognised **Role** bundles nothing.
fn role_permissions(role: &str) -> Vec<&'static str> {
    /// Read access across the freight resources.
    const VIEWER: &[&str] = &[
        "freight.shippers.get",
        "freight.shippers.list",
        "freight.sites.get",
        "freight.sites.list",
        "freight.shipments.get",
        "freight.shipments.list",
    ];
    /// The write verbs `roles/freight.editor` adds on top of viewer.
    const EDITOR_EXTRA: &[&str] = &[
        "freight.shippers.create",
        "freight.shippers.update",
        "freight.shippers.delete",
        "freight.sites.create",
        "freight.sites.update",
        "freight.sites.delete",
        "freight.shipments.create",
        "freight.shipments.update",
        "freight.shipments.delete",
    ];
    /// IAM-policy administration `roles/freight.admin` adds on top of editor.
    const ADMIN_EXTRA: &[&str] = &[
        "freight.shippers.getIamPolicy",
        "freight.shippers.setIamPolicy",
    ];
    match role {
        "roles/freight.viewer" => VIEWER.to_vec(),
        "roles/freight.editor" => [VIEWER, EDITOR_EXTRA].concat(),
        "roles/freight.admin" => [VIEWER, EDITOR_EXTRA, ADMIN_EXTRA].concat(),
        _ => Vec::new(),
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
        request: Request<TestIamPermissionsRequest>,
    ) -> Result<Response<TestIamPermissionsResponse>, Status> {
        // The caller identity is the example-owned credential the AIP-211 read gate
        // also reads (a real server derives the principal from authenticated
        // transport); an absent or unparseable header is an anonymous caller.
        let caller = caller_member(request.metadata());
        let req = request.into_inner();
        if req.resource.is_empty() {
            return Err(Status::invalid_argument("resource is required"));
        }

        // Validate the requested **Permissions** up front: each must be a
        // well-formed `service.resource.verb` (the IAM contract disallows
        // wildcards). A malformed one converts via aip-iam's AIP-193 `From<Error>`
        // (#16) to `INVALID_ARGUMENT` — distinct from a valid permission the caller
        // simply does not hold, which is omitted from the response, never errored.
        let requested = req
            .permissions
            .iter()
            .map(|permission| permission.parse::<Permission>())
            .collect::<Result<Vec<_>, _>>()?;

        // Decide the held subset *through* the eval adapter (#66): the requested
        // permissions intersected with the set the caller actually holds.
        let granted = self.granted_permissions(&req.resource, caller.as_ref())?;
        let permissions = requested
            .into_iter()
            .filter(|permission| granted.contains(permission))
            .map(|permission| permission.to_string())
            .collect();

        Ok(Response::new(TestIamPermissionsResponse { permissions }))
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

    /// A version-3 policy granting `role` to `members` gated by the CEL
    /// `expression` **Condition** — the conditional grant `TestIamPermissions`
    /// honours through the eval adapter.
    fn conditional_policy(role: &str, members: &[&str], expression: &str) -> Policy {
        let mut policy = policy(role, members);
        policy.version = 3;
        policy.bindings[0].condition = Some(Expr {
            expression: expression.to_owned(),
            ..Expr::default()
        });
        policy
    }

    /// Drive `TestIamPermissions` for `RESOURCE` as `caller` (an `x-freight-caller`
    /// metadata value, or anonymous when `None`), returning the held subset.
    async fn test_perms(
        server: &IamServer,
        caller: Option<&str>,
        permissions: &[&str],
    ) -> Vec<String> {
        test_perms_on(server, caller, RESOURCE, permissions)
            .await
            .expect("test_iam_permissions succeeds")
    }

    /// Like [`test_perms`] but against an arbitrary `resource` and surfacing the
    /// `Status` (so a test can assert the error path).
    async fn test_perms_on(
        server: &IamServer,
        caller: Option<&str>,
        resource: &str,
        permissions: &[&str],
    ) -> Result<Vec<String>, Status> {
        let mut request = Request::new(TestIamPermissionsRequest {
            resource: resource.to_owned(),
            permissions: permissions.iter().map(|p| (*p).to_owned()).collect(),
        });
        if let Some(caller) = caller {
            request.metadata_mut().insert(
                crate::service::CALLER_METADATA_KEY,
                caller.parse().expect("a valid caller metadata value"),
            );
        }
        server
            .test_iam_permissions(request)
            .await
            .map(|response| response.into_inner().permissions)
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

    #[tokio::test]
    async fn set_then_get_round_trips_a_conditional_binding() {
        // The storage change behind #68: a conditional version-3 Binding now
        // round-trips its Condition (its expression) rather than being dropped, and
        // `version` is reconstructed from the conditions⟹version-3 invariant.
        let server = IamServer::new();
        let expression = "request.time < timestamp(\"2030-01-01T00:00:00Z\")";
        let stored = set(
            &server,
            conditional_policy(
                "roles/freight.viewer",
                &["user:alice@example.com"],
                expression,
            ),
        )
        .await
        .expect("a well-formed version-3 conditional policy is accepted");

        assert_eq!(
            stored.version, 3,
            "a conditional policy reconstructs as version 3"
        );
        assert_eq!(
            stored.bindings[0]
                .condition
                .as_ref()
                .expect("the Condition survives storage")
                .expression,
            expression,
        );
        assert_eq!(
            get(&server, RESOURCE).await,
            stored,
            "get round-trips the conditional policy, etag and all",
        );
    }

    #[tokio::test]
    async fn returns_only_the_held_subset_of_the_requested_permissions() {
        // The viewer role bundles the read verbs but not delete, so only the held
        // permission comes back — the unheld one is omitted, not an error.
        let server = IamServer::new();
        set(
            &server,
            policy("roles/freight.viewer", &["user:alice@example.com"]),
        )
        .await
        .expect("policy accepted");

        let held = test_perms(
            &server,
            Some("user:alice@example.com"),
            &["freight.shippers.get", "freight.shippers.delete"],
        )
        .await;
        assert_eq!(held, vec!["freight.shippers.get"]);
    }

    #[tokio::test]
    async fn the_held_subset_changes_with_the_policy() {
        // Re-binding alice from viewer to editor — a role the catalogue expands to
        // the write verbs too — widens the held subset to include delete.
        let server = IamServer::new();
        let requested = ["freight.shippers.get", "freight.shippers.delete"];

        set(
            &server,
            policy("roles/freight.viewer", &["user:alice@example.com"]),
        )
        .await
        .expect("viewer policy accepted");
        assert_eq!(
            test_perms(&server, Some("user:alice@example.com"), &requested).await,
            vec!["freight.shippers.get"],
        );

        set(
            &server,
            policy("roles/freight.editor", &["user:alice@example.com"]),
        )
        .await
        .expect("editor policy accepted");
        assert_eq!(
            test_perms(&server, Some("user:alice@example.com"), &requested).await,
            vec!["freight.shippers.get", "freight.shippers.delete"],
        );
    }

    #[tokio::test]
    async fn a_caller_in_no_binding_holds_nothing() {
        // bob is named in no Binding, so he holds none of the requested permissions
        // — an empty subset, never an error.
        let server = IamServer::new();
        set(
            &server,
            policy("roles/freight.viewer", &["user:alice@example.com"]),
        )
        .await
        .expect("policy accepted");

        let held = test_perms(
            &server,
            Some("user:bob@example.com"),
            &["freight.shippers.get"],
        )
        .await;
        assert!(held.is_empty(), "a non-member holds nothing");
    }

    #[tokio::test]
    async fn an_unprotected_resource_holds_nothing() {
        // Unlike the read gate's "no Policy ⇒ public" simplification,
        // TestIamPermissions decides purely from the Policy: a resource with none
        // grants nothing.
        let server = IamServer::new();
        let held = test_perms_on(
            &server,
            Some("user:alice@example.com"),
            "shippers/unprotected",
            &["freight.shippers.get"],
        )
        .await
        .expect("test_iam_permissions succeeds");
        assert!(held.is_empty(), "an unprotected resource holds nothing");
    }

    #[tokio::test]
    async fn a_condition_that_holds_keeps_the_binding() {
        // A conditional Binding is honoured through the eval adapter: a Condition
        // that holds (the time window is open) keeps its permissions.
        let server = IamServer::new();
        set(
            &server,
            conditional_policy(
                "roles/freight.viewer",
                &["user:alice@example.com"],
                "request.time < timestamp(\"2030-01-01T00:00:00Z\")",
            ),
        )
        .await
        .expect("conditional policy accepted");

        let held = test_perms(
            &server,
            Some("user:alice@example.com"),
            &["freight.shippers.get"],
        )
        .await;
        assert_eq!(held, vec!["freight.shippers.get"]);
    }

    #[tokio::test]
    async fn a_condition_that_fails_excludes_the_binding() {
        // The same grant gated by a Condition that does *not* hold (the window has
        // closed) excludes the Binding's permissions entirely.
        let server = IamServer::new();
        set(
            &server,
            conditional_policy(
                "roles/freight.viewer",
                &["user:alice@example.com"],
                "request.time < timestamp(\"2020-01-01T00:00:00Z\")",
            ),
        )
        .await
        .expect("conditional policy accepted");

        let held = test_perms(
            &server,
            Some("user:alice@example.com"),
            &["freight.shippers.get"],
        )
        .await;
        assert!(
            held.is_empty(),
            "a failed Condition excludes the permission"
        );
    }

    #[tokio::test]
    async fn a_condition_reads_the_resource_from_the_request_context() {
        // The Condition environment carries `resource.name` from the request, so a
        // Condition can gate on the resource under test — here it matches RESOURCE,
        // so the permission is held.
        let server = IamServer::new();
        set(
            &server,
            conditional_policy(
                "roles/freight.viewer",
                &["user:alice@example.com"],
                "resource.name == \"shippers/acme\"",
            ),
        )
        .await
        .expect("conditional policy accepted");

        let held = test_perms(
            &server,
            Some("user:alice@example.com"),
            &["freight.shippers.get"],
        )
        .await;
        assert_eq!(held, vec!["freight.shippers.get"]);
    }

    #[tokio::test]
    async fn rejects_a_malformed_requested_permission() {
        use tonic_types::StatusExt as _;

        // A requested permission that is not a well-formed `service.resource.verb`
        // is a bad request, distinct from a valid-but-unheld permission: it is
        // rejected via aip-iam's AIP-193 mapping (INVALID_ARGUMENT, IAM_* reason),
        // before any Policy lookup.
        let server = IamServer::new();
        let status = test_perms_on(
            &server,
            Some("user:alice@example.com"),
            RESOURCE,
            &["freight.shippers"],
        )
        .await
        .expect_err("a malformed permission is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(
            status
                .get_details_error_info()
                .expect("an ErrorInfo is attached (AIP-193)")
                .reason,
            "IAM_PERMISSION_MALFORMED",
        );
    }

    #[tokio::test]
    async fn a_broken_stored_condition_is_an_error_not_a_silent_deny() {
        use tonic_types::StatusExt as _;

        // A stored Condition that is not valid CEL surfaces through the eval
        // adapter as INVALID_ARGUMENT (IAM_CONDITION_MALFORMED) when the matched
        // caller forces its evaluation — the adapter keeps a broken Condition
        // distinct from one that simply did not hold, never a silent denial.
        let server = IamServer::new();
        set(
            &server,
            conditional_policy("roles/freight.viewer", &["user:alice@example.com"], "1 +"),
        )
        .await
        .expect("the version-3 invariant passes; CEL validity is not checked at set");

        let status = test_perms_on(
            &server,
            Some("user:alice@example.com"),
            RESOURCE,
            &["freight.shippers.get"],
        )
        .await
        .expect_err("a malformed stored Condition is surfaced");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(
            status
                .get_details_error_info()
                .expect("an ErrorInfo is attached (AIP-193)")
                .reason,
            "IAM_CONDITION_MALFORMED",
        );
    }
}
