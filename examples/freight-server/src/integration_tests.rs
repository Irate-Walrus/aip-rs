//! In-process integration tests driving the README `grpcurl` journeys end-to-end.
//!
//! Each test boots a real [`FreightServer`] and [`IamServer`] sharing a single
//! [`PolicyStore`] and executes the multi-step flows documented in the README
//! — Shipper CRUD with update_mask, ListSites ordering / filtering / pagination
//! with the request-checksum guard, the IAMPolicy read-modify-write etag dance,
//! TestIamPermissions, and the AIP-211 non-leaking denial. The service methods
//! are called via `tonic::Request` / `tonic::Response`, exactly as a real tonic
//! client would call them.

#![cfg(test)]

use std::sync::Arc;

use tonic::Request;

use crate::iam::IamServer;
use crate::proto::einride::example::freight::v1::{
    freight_service_server::FreightService, CreateShipmentRequest, CreateShipperRequest,
    CreateSiteRequest, DeleteShipperRequest, GetShipperRequest, ListShipmentsRequest,
    ListShippersRequest, ListSitesRequest, Shipment, Shipper, Site, UpdateShipperRequest,
};
use crate::proto::google::iam::v1::{
    iam_policy_server::IamPolicy, GetIamPolicyRequest, SetIamPolicyRequest,
    TestIamPermissionsRequest,
};
use crate::service::{FreightServer, CALLER_METADATA_KEY};
use crate::storage::PolicyStore;
use aip::iam::proto::{google::r#type::Expr, Binding, Policy};

// ─── Test fixtures ────────────────────────────────────────────────────────────

/// A shipper parent used across the Sites and Shipments journeys.
const PARENT: &str = "shippers/acme";

/// Create a fresh `(FreightServer, IamServer)` pair sharing one `PolicyStore`.
fn make_server() -> (FreightServer, IamServer) {
    let policies = Arc::new(PolicyStore::new());
    let freight = FreightServer::with_policies(Arc::clone(&policies));
    let iam = IamServer::with_store(policies);
    (freight, iam)
}

/// Build a `tonic::Request` carrying the `x-freight-caller` metadata header so
/// the `FreightService` AIP-211 authorization gate and `TestIamPermissions` see
/// the given principal.
fn as_caller<T>(caller: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    request.metadata_mut().insert(
        CALLER_METADATA_KEY,
        caller.parse().expect("valid metadata value"),
    );
    request
}

/// A version-1 `Policy` granting `role` to `members` with no etag.
fn policy_v1(role: &str, members: &[&str]) -> Policy {
    Policy {
        version: 1,
        bindings: vec![Binding {
            role: role.to_owned(),
            members: members.iter().map(|m| m.to_string()).collect(),
            condition: None,
        }],
        etag: Vec::new(),
        audit_configs: Vec::new(),
    }
}

/// A version-3 `Policy` granting `role` to `members` behind a CEL `expression`.
fn policy_v3_conditional(role: &str, members: &[&str], expression: &str) -> Policy {
    Policy {
        version: 3,
        bindings: vec![Binding {
            role: role.to_owned(),
            members: members.iter().map(|m| m.to_string()).collect(),
            condition: Some(Expr {
                expression: expression.to_owned(),
                ..Expr::default()
            }),
        }],
        etag: Vec::new(),
        audit_configs: Vec::new(),
    }
}

/// Create a site under [`PARENT`] with the given display name.
async fn seed_site(freight: &FreightServer, display_name: &str) {
    freight
        .create_site(Request::new(CreateSiteRequest {
            parent: PARENT.to_owned(),
            site: Some(Site {
                display_name: display_name.to_owned(),
                ..Default::default()
            }),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect("create_site succeeds");
}

// ─── Shipper CRUD with update_mask ────────────────────────────────────────────

/// README flow: `CreateShipper` mints a system-assigned name, `ListShippers`
/// returns it, `GetShipper` fetches it, `UpdateShipper` with an `update_mask`
/// patches only the named field while leaving OUTPUT_ONLY timestamps intact,
/// and `DeleteShipper` removes it. Every step that can fail with an
/// `INVALID_ARGUMENT` asserts the full AIP-193 details (ErrorInfo + BadRequest).
#[tokio::test]
async fn shipper_crud_with_update_mask() {
    use tonic_types::StatusExt as _;

    let (freight, _iam) = make_server();

    // CreateShipper mints a system-assigned name (a UUIDv4, AIP-148).
    let created = freight
        .create_shipper(Request::new(CreateShipperRequest {
            shipper: Some(Shipper {
                display_name: "Acme".to_owned(),
                ..Default::default()
            }),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect("create_shipper succeeds")
        .into_inner();
    assert!(!created.name.is_empty(), "system-assigned name was minted");
    assert!(
        created.create_time.is_some() && created.update_time.is_some(),
        "server-set timestamps are populated"
    );

    // ListShippers shows exactly the one shipper.
    let listed = freight
        .list_shippers(Request::new(ListShippersRequest::default()))
        .await
        .expect("list_shippers succeeds")
        .into_inner();
    assert_eq!(listed.shippers.len(), 1);
    assert_eq!(listed.shippers[0].name, created.name);
    assert_eq!(listed.next_page_token, "");

    // GetShipper retrieves it.
    let got = freight
        .get_shipper(Request::new(GetShipperRequest {
            name: created.name.clone(),
        }))
        .await
        .expect("get_shipper succeeds")
        .into_inner();
    assert_eq!(got, created);

    // CreateShipper stamps an AIP-154 content etag the client echoes back to make
    // the read-modify-write safe (#93).
    assert!(!created.etag.is_empty(), "create stamps a content etag");

    // UpdateShipper with an AIP-134 update_mask: only `display_name` is named,
    // so it changes while the rest of the stored shipper (including OUTPUT_ONLY
    // `create_time`) is left untouched. The etag just read is piggybacked back for
    // the AIP-154 freshness check (#93), and the response carries a fresh one.
    let updated = freight
        .update_shipper(Request::new(UpdateShipperRequest {
            shipper: Some(Shipper {
                name: created.name.clone(),
                display_name: "Acme Corp".to_owned(),
                etag: created.etag.clone(),
                ..Default::default()
            }),
            update_mask: Some(prost_types::FieldMask {
                paths: vec!["display_name".to_owned()],
            }),
            ..Default::default()
        }))
        .await
        .expect("update_shipper succeeds")
        .into_inner();
    assert_eq!(updated.display_name, "Acme Corp");
    assert_eq!(
        updated.create_time, created.create_time,
        "OUTPUT_ONLY create_time must not change"
    );
    assert_ne!(
        updated.update_time, created.update_time,
        "update_time must advance after a write"
    );
    assert_ne!(
        updated.etag, created.etag,
        "the content changed, so the etag advances"
    );

    // Masking `display_name` while the request carries no value would blank a
    // REQUIRED field — the `fieldbehavior` primitive rejects it with
    // INVALID_ARGUMENT + AIP-193 details (BadRequest + ErrorInfo, domain aip-rs).
    let status = freight
        .update_shipper(Request::new(UpdateShipperRequest {
            shipper: Some(Shipper {
                name: created.name.clone(),
                ..Default::default()
            }),
            update_mask: Some(prost_types::FieldMask {
                paths: vec!["display_name".to_owned()],
            }),
            ..Default::default()
        }))
        .await
        .expect_err("blanking a REQUIRED field is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached (AIP-193 MUST)");
    assert_eq!(info.reason, "FIELD_REQUIRED");
    assert_eq!(info.domain, "aip-rs");
    let bad = status
        .get_details_bad_request()
        .expect("BadRequest is attached");
    assert_eq!(bad.field_violations[0].field, "display_name");

    // DeleteShipper carries the current etag on the request (it can't piggyback on
    // the resource); the fresh one permits the delete (#93). A subsequent
    // GetShipper is NOT_FOUND.
    freight
        .delete_shipper(Request::new(DeleteShipperRequest {
            name: created.name.clone(),
            etag: updated.etag.clone(),
        }))
        .await
        .expect("delete_shipper succeeds");

    let status = freight
        .get_shipper(Request::new(GetShipperRequest {
            name: created.name.clone(),
        }))
        .await
        .expect_err("deleted shipper is not found");
    assert_eq!(status.code(), tonic::Code::NotFound);
}

/// README flow: `CreateShipper` with an empty `shipper` body (no `display_name`)
/// is rejected by the `fieldbehavior` primitive with INVALID_ARGUMENT, an
/// `ErrorInfo` (reason `FIELD_REQUIRED`, domain `aip-rs`), and a `BadRequest`
/// naming the field path — the AIP-193 "MUST include error details" contract.
#[tokio::test]
async fn create_shipper_missing_display_name_aip193_details() {
    use tonic_types::StatusExt as _;

    let (freight, _iam) = make_server();

    let status = freight
        .create_shipper(Request::new(CreateShipperRequest {
            shipper: Some(Shipper::default()),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect_err("an empty display_name is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached (AIP-193 MUST)");
    assert_eq!(info.reason, "FIELD_REQUIRED");
    assert_eq!(info.domain, "aip-rs");

    let bad = status
        .get_details_bad_request()
        .expect("BadRequest is attached");
    assert_eq!(bad.field_violations.len(), 1);
    assert_eq!(bad.field_violations[0].field, "display_name");
}

// ─── ListSites ordering / pagination / checksum guard ─────────────────────────

/// README flow: seed two sites, `ListSites` with `order_by` ascending and
/// descending, paginate with `page_size=1` collecting both pages, then verify
/// the request-checksum guard rejects a token when `order_by` or `filter`
/// changes mid-pagination.
#[tokio::test]
async fn list_sites_ordering_and_pagination_with_checksum_guard() {
    let (freight, _iam) = make_server();

    for name in ["Bravo", "Alpha", "Charlie", "Delta", "Echo"] {
        seed_site(&freight, name).await;
    }

    // `order_by = "display_name"` sorts ascending.
    let resp = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "display_name".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("list_sites asc succeeds")
        .into_inner();
    let names: Vec<&str> = resp.sites.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]);

    // `order_by = "display_name desc"` sorts descending.
    let resp = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "display_name desc".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("list_sites desc succeeds")
        .into_inner();
    let names: Vec<&str> = resp.sites.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, ["Echo", "Delta", "Charlie", "Bravo", "Alpha"]);

    // Pagination with page_size=1: each page carries one site and a
    // `next_page_token`; the last page has an empty token. The concatenation
    // across pages equals the full sorted listing.
    let mut all_names = Vec::new();
    let mut page_token = String::new();
    loop {
        let resp = freight
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                order_by: "display_name".to_owned(),
                page_size: 1,
                page_token: page_token.clone(),
                ..Default::default()
            }))
            .await
            .expect("list_sites page succeeds")
            .into_inner();
        all_names.extend(resp.sites.into_iter().map(|s| s.display_name));
        page_token = resp.next_page_token;
        if page_token.is_empty() {
            break;
        }
    }
    assert_eq!(all_names, ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]);

    // Mint a page token under order_by="display_name".
    let first = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "display_name".to_owned(),
            page_size: 2,
            ..Default::default()
        }))
        .await
        .expect("first page succeeds")
        .into_inner();
    assert!(!first.next_page_token.is_empty(), "more pages follow");

    // Replaying the token under a different order_by flips the request checksum
    // and the stale token is rejected with INVALID_ARGUMENT (AIP-158 checksum
    // guard, #7): `order_by` is a non-pagination field.
    let status = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "name".to_owned(), // changed mid-pagination
            page_size: 2,
            page_token: first.next_page_token.clone(),
            ..Default::default()
        }))
        .await
        .expect_err("changing order_by mid-pagination rejects the token");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    // Adding a `filter` mid-pagination also flips the checksum: `filter` is a
    // non-pagination field, so the guard covers it too.
    let status = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "display_name".to_owned(),
            page_size: 2,
            page_token: first.next_page_token,
            filter: r#"display_name = "Alpha""#.to_owned(),
            ..Default::default()
        }))
        .await
        .expect_err("adding a filter mid-pagination rejects the token");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
}

/// README flow: `ListSites` with AIP-160 `filter` expressions — equality,
/// disjunction, and the `has` operator over display_name, annotations, and tags.
/// An unknown `order_by` field is rejected with INVALID_ARGUMENT carrying AIP-193
/// details (ErrorInfo reason `ORDER_BY_UNKNOWN_FIELD`, BadRequest naming the field).
/// A `CreateSite` missing the required `display_name` is rejected with the
/// service's own domain (`freight.example.com`).
#[tokio::test]
async fn list_sites_aip160_filtering_and_error_details() {
    use tonic_types::StatusExt as _;

    let (freight, _iam) = make_server();

    for name in ["Alpha", "Bravo", "Charlie"] {
        seed_site(&freight, name).await;
    }

    // A server-side predicate composition test: the user `filter` is conjoined
    // with the parent scope so only sites under this parent are returned.
    let resp = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            filter: r#"display_name = "Alpha""#.to_owned(),
            order_by: "display_name".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("equality filter succeeds")
        .into_inner();
    let names: Vec<&str> = resp.sites.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, ["Alpha"]);

    // Disjunction: `OR` returns the union of both branches.
    let resp = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            filter: r#"display_name = "Alpha" OR display_name = "Charlie""#.to_owned(),
            order_by: "display_name".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("OR filter succeeds")
        .into_inner();
    let names: Vec<&str> = resp.sites.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, ["Alpha", "Charlie"]);

    // Has operator on a string is a substring match: `display_name:lph` keeps
    // only sites whose display name contains "lph".
    let resp = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            filter: "display_name:lph".to_owned(),
            order_by: "display_name".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("has-operator filter succeeds")
        .into_inner();
    let names: Vec<&str> = resp.sites.iter().map(|s| s.display_name.as_str()).collect();
    assert_eq!(names, ["Alpha"]);

    // An unknown `order_by` field is rejected by the AIP-132 `ordering` crate
    // (#9/#16): INVALID_ARGUMENT with an ErrorInfo (reason ORDER_BY_UNKNOWN_FIELD,
    // domain aip-rs) and a BadRequest naming the offending path.
    let status = freight
        .list_sites(Request::new(ListSitesRequest {
            parent: PARENT.to_owned(),
            order_by: "bogus_field".to_owned(),
            ..Default::default()
        }))
        .await
        .expect_err("unknown order_by field is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached (AIP-193 MUST)");
    assert_eq!(info.reason, "ORDER_BY_UNKNOWN_FIELD");
    assert_eq!(info.domain, "aip-rs");
    let bad = status
        .get_details_bad_request()
        .expect("BadRequest is attached");
    assert_eq!(bad.field_violations[0].field, "bogus_field");

    // A `CreateSite` missing `display_name` is rejected by the server's own
    // presence check — no aip-rs primitive covers it — through a `Validator`
    // that carries the service's own AIP-193 domain (`freight.example.com`).
    let status = freight
        .create_site(Request::new(CreateSiteRequest {
            parent: PARENT.to_owned(),
            site: Some(Site::default()),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect_err("a site without display_name is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "FIELD_REQUIRED");
    assert_eq!(info.domain, "freight.example.com");
}

// ─── Shipments journey ────────────────────────────────────────────────────────

/// README flow: seed two shipments, `ListShipments` with no filter, then with
/// `origin_site = …` and `annotations:priority` filters. A `CreateShipment`
/// missing both endpoints collects both violations into one `BadRequest`
/// (the `Validator` accumulation path, AIP-193).
#[tokio::test]
async fn list_shipments_filtering_and_missing_endpoints_aip193() {
    use tonic_types::StatusExt as _;

    let (freight, _iam) = make_server();

    let site_a = format!("{PARENT}/sites/a");
    let site_b = format!("{PARENT}/sites/b");

    // Seed two shipments between the two sites.
    for (origin, dest, ann) in [
        (
            site_a.as_str(),
            site_b.as_str(),
            &[("priority", "high")][..],
        ),
        (site_b.as_str(), site_a.as_str(), &[("region", "west")][..]),
    ] {
        freight
            .create_shipment(Request::new(CreateShipmentRequest {
                parent: PARENT.to_owned(),
                shipment: Some(Shipment {
                    origin_site: origin.to_owned(),
                    destination_site: dest.to_owned(),
                    annotations: ann
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    ..Default::default()
                }),
                request_id: String::new(),
                ..Default::default()
            }))
            .await
            .expect("create_shipment succeeds");
    }

    // No filter: both shipments in the parent scope are listed.
    let resp = freight
        .list_shipments(Request::new(ListShipmentsRequest {
            parent: PARENT.to_owned(),
            ..Default::default()
        }))
        .await
        .expect("list_shipments with no filter succeeds")
        .into_inner();
    assert_eq!(resp.shipments.len(), 2);

    // Filter: `origin_site = "…/sites/a"` returns only the matching shipment.
    let resp = freight
        .list_shipments(Request::new(ListShipmentsRequest {
            parent: PARENT.to_owned(),
            filter: format!(r#"origin_site = "{site_a}""#),
            ..Default::default()
        }))
        .await
        .expect("origin_site filter succeeds")
        .into_inner();
    let origins: Vec<&str> = resp
        .shipments
        .iter()
        .map(|s| s.origin_site.as_str())
        .collect();
    assert_eq!(origins, [site_a.as_str()]);

    // Has operator over the `annotations` map: `annotations:priority` returns only
    // the shipment carrying that key (via SQLite `json_each`).
    let resp = freight
        .list_shipments(Request::new(ListShipmentsRequest {
            parent: PARENT.to_owned(),
            filter: "annotations:priority".to_owned(),
            ..Default::default()
        }))
        .await
        .expect("annotations:key filter succeeds")
        .into_inner();
    assert_eq!(resp.shipments.len(), 1);
    assert_eq!(resp.shipments[0].origin_site, site_a);

    // A `CreateShipment` missing both endpoints accumulates both violations into
    // one `BadRequest` — a `Validator` collects them so the client gets all of
    // them in a single response (AIP-193 + freight service domain).
    let status = freight
        .create_shipment(Request::new(CreateShipmentRequest {
            parent: PARENT.to_owned(),
            shipment: Some(Shipment::default()),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect_err("a shipment missing both endpoints is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let bad = status
        .get_details_bad_request()
        .expect("BadRequest is attached");
    let fields: Vec<&str> = bad
        .field_violations
        .iter()
        .map(|v| v.field.as_str())
        .collect();
    assert_eq!(
        fields,
        ["shipment.origin_site", "shipment.destination_site"],
        "both missing endpoints appear in one BadRequest"
    );
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "FIELD_REQUIRED");
    assert_eq!(info.domain, "freight.example.com");
}

// ─── IAMPolicy read-modify-write etag dance ───────────────────────────────────

/// README flow: `GetIamPolicy` on an unset resource returns the empty `Policy`;
/// `SetIamPolicy` (no etag) stamps a fresh etag and normalises member order;
/// `GetIamPolicy` round-trips the stored `Policy`; a second `SetIamPolicy`
/// (with the current etag) advances the etag; replaying the now-stale etag is
/// rejected with `ABORTED` (IAM_POLICY_ETAG_MISMATCH); a malformed Member is
/// rejected with INVALID_ARGUMENT (IAM_MEMBER_UNKNOWN_TYPE); a conditional
/// Binding on version 1 is rejected (IAM_POLICY_CONDITION_REQUIRES_VERSION_3).
#[tokio::test]
async fn iam_policy_read_modify_write_etag_dance() {
    use tonic_types::StatusExt as _;

    let (_freight, iam) = make_server();
    let resource = "shippers/acme".to_owned();

    // `GetIamPolicy` on a resource with no policy is not an error — it returns
    // the empty `Policy` (the IAM `GetIamPolicy` contract).
    let empty = iam
        .get_iam_policy(Request::new(GetIamPolicyRequest {
            resource: resource.clone(),
            options: None,
        }))
        .await
        .expect("get on unset resource succeeds")
        .into_inner();
    assert_eq!(empty, Policy::default());

    // A first `SetIamPolicy` may omit the etag: the server accepts it, normalises
    // member order (canonical: group before user), and stamps a fresh etag.
    let stored = iam
        .set_iam_policy(Request::new(SetIamPolicyRequest {
            resource: resource.clone(),
            policy: Some(policy_v1(
                "roles/viewer",
                &["user:alice@example.com", "group:ops@example.com"],
            )),
            update_mask: None,
        }))
        .await
        .expect("first SetIamPolicy is accepted")
        .into_inner();
    assert!(!stored.etag.is_empty(), "server stamps a fresh etag");
    assert_eq!(
        stored.bindings[0].members,
        ["group:ops@example.com", "user:alice@example.com"],
        "members are in canonical order after normalisation"
    );

    // `GetIamPolicy` returns exactly what was stored.
    let got = iam
        .get_iam_policy(Request::new(GetIamPolicyRequest {
            resource: resource.clone(),
            options: None,
        }))
        .await
        .expect("get_iam_policy succeeds")
        .into_inner();
    assert_eq!(got, stored);

    // Read-modify-write: sending the current etag back is accepted; the stored
    // etag advances (each write produces a fresh content digest).
    let second = iam
        .set_iam_policy(Request::new(SetIamPolicyRequest {
            resource: resource.clone(),
            policy: Some(Policy {
                etag: stored.etag.clone(),
                ..policy_v1("roles/viewer", &["user:alice@example.com"])
            }),
            update_mask: None,
        }))
        .await
        .expect("matching etag is accepted")
        .into_inner();
    assert_ne!(second.etag, stored.etag, "etag advances on each write");
    assert_eq!(
        second.bindings[0].members,
        ["user:alice@example.com"],
        "second write's binding content is stored correctly"
    );

    // Replaying the now-stale etag is rejected with `ABORTED` — the IAM
    // optimistic-concurrency contract. The stale write must not take effect.
    let status = iam
        .set_iam_policy(Request::new(SetIamPolicyRequest {
            resource: resource.clone(),
            policy: Some(Policy {
                etag: stored.etag.clone(), // stale
                ..policy_v1("roles/editor", &["user:bob@example.com"])
            }),
            update_mask: None,
        }))
        .await
        .expect_err("stale etag is rejected");
    assert_eq!(status.code(), tonic::Code::Aborted);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "IAM_POLICY_ETAG_MISMATCH");

    // Verify the stale write did not take effect: the second write still stands.
    let unchanged = iam
        .get_iam_policy(Request::new(GetIamPolicyRequest {
            resource: resource.clone(),
            options: None,
        }))
        .await
        .expect("get after rejected write succeeds")
        .into_inner();
    assert_eq!(unchanged, second);

    // A malformed Member is rejected with INVALID_ARGUMENT + IAM_* ErrorInfo
    // (reason IAM_MEMBER_UNKNOWN_TYPE, domain aip-rs).
    let status = iam
        .set_iam_policy(Request::new(SetIamPolicyRequest {
            resource: resource.clone(),
            policy: Some(policy_v1("roles/viewer", &["robot:r2d2"])),
            update_mask: None,
        }))
        .await
        .expect_err("malformed Member is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "IAM_MEMBER_UNKNOWN_TYPE");
    assert_eq!(info.domain, "aip-rs");

    // A conditional Binding requires policy version 3 — version 1 is
    // INVALID_ARGUMENT (IAM_POLICY_CONDITION_REQUIRES_VERSION_3).
    let mut conditional = policy_v1("roles/viewer", &["user:alice@example.com"]);
    conditional.bindings[0].condition = Some(Expr {
        expression: r#"request.time < timestamp("2030-01-01T00:00:00Z")"#.to_owned(),
        ..Expr::default()
    });
    let status = iam
        .set_iam_policy(Request::new(SetIamPolicyRequest {
            resource: resource.clone(),
            policy: Some(conditional),
            update_mask: None,
        }))
        .await
        .expect_err("conditional binding on version 1 is rejected");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "IAM_POLICY_CONDITION_REQUIRES_VERSION_3");
}

// ─── TestIamPermissions ───────────────────────────────────────────────────────

/// README flow: grant alice `roles/freight.viewer`; she holds the read verb but
/// not delete. bob is in no Binding — empty subset, no error. Rebind alice to
/// `roles/freight.editor`; the held subset widens to include delete. A
/// conditional Binding that holds keeps the permission; one that has already
/// expired excludes it.
#[tokio::test]
async fn test_iam_permissions_journey() {
    let (_freight, iam) = make_server();
    let resource = "shippers/acme".to_owned();

    // Grant alice the freight viewer role.
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: resource.clone(),
        policy: Some(policy_v1(
            "roles/freight.viewer",
            &["user:alice@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("viewer policy accepted");

    // alice holds the read verb but not delete.
    let held = iam
        .test_iam_permissions(as_caller(
            "user:alice@example.com",
            TestIamPermissionsRequest {
                resource: resource.clone(),
                permissions: vec![
                    "freight.shippers.get".to_owned(),
                    "freight.shippers.delete".to_owned(),
                ],
            },
        ))
        .await
        .expect("test_iam_permissions succeeds")
        .into_inner()
        .permissions;
    assert_eq!(held, ["freight.shippers.get"]);

    // bob is named in no Binding — empty subset, never an error.
    let held = iam
        .test_iam_permissions(as_caller(
            "user:bob@example.com",
            TestIamPermissionsRequest {
                resource: resource.clone(),
                permissions: vec!["freight.shippers.get".to_owned()],
            },
        ))
        .await
        .expect("test for non-member succeeds")
        .into_inner()
        .permissions;
    assert!(held.is_empty(), "a non-member holds nothing");

    // Rebind alice from viewer to editor: the editor role bundles the write
    // verbs too, so delete now comes back as well.
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: resource.clone(),
        policy: Some(policy_v1(
            "roles/freight.editor",
            &["user:alice@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("editor policy accepted");

    let held = iam
        .test_iam_permissions(as_caller(
            "user:alice@example.com",
            TestIamPermissionsRequest {
                resource: resource.clone(),
                permissions: vec![
                    "freight.shippers.get".to_owned(),
                    "freight.shippers.delete".to_owned(),
                ],
            },
        ))
        .await
        .expect("test with editor policy succeeds")
        .into_inner()
        .permissions;
    assert_eq!(held, ["freight.shippers.get", "freight.shippers.delete"]);

    // A conditional Binding whose Condition holds (the time window is still open)
    // keeps the Binding's permissions.
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: resource.clone(),
        policy: Some(policy_v3_conditional(
            "roles/freight.viewer",
            &["user:alice@example.com"],
            r#"request.time < timestamp("2030-01-01T00:00:00Z")"#,
        )),
        update_mask: None,
    }))
    .await
    .expect("conditional policy accepted");

    let held = iam
        .test_iam_permissions(as_caller(
            "user:alice@example.com",
            TestIamPermissionsRequest {
                resource: resource.clone(),
                permissions: vec!["freight.shippers.get".to_owned()],
            },
        ))
        .await
        .expect("test with holding condition succeeds")
        .into_inner()
        .permissions;
    assert_eq!(held, ["freight.shippers.get"]);

    // The same grant gated by a Condition whose window has already closed excludes
    // the Binding's permissions — the held subset is empty.
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: resource.clone(),
        policy: Some(policy_v3_conditional(
            "roles/freight.viewer",
            &["user:alice@example.com"],
            r#"request.time < timestamp("2020-01-01T00:00:00Z")"#,
        )),
        update_mask: None,
    }))
    .await
    .expect("expired-window conditional policy accepted");

    let held = iam
        .test_iam_permissions(as_caller(
            "user:alice@example.com",
            TestIamPermissionsRequest {
                resource: resource.clone(),
                permissions: vec!["freight.shippers.get".to_owned()],
            },
        ))
        .await
        .expect("test with failing condition succeeds")
        .into_inner()
        .permissions;
    assert!(
        held.is_empty(),
        "a failed Condition excludes the permission"
    );
}

// ─── AIP-211 non-leaking denial ───────────────────────────────────────────────

/// README flow: create a shipper, lock it to alice; she reads it. bob (and an
/// anonymous caller) get the canonical non-leaking PERMISSION_DENIED — the
/// message hides whether the resource exists. A never-created shipper with a
/// locked Policy also yields PERMISSION_DENIED for bob, proving the denial is
/// indistinguishable from the existing-but-forbidden case. When bob is granted on
/// the parent collection, the missing name comes back NOT_FOUND instead — the
/// AIP-211 fallback that reveals the gap only to an authorized parent reader.
#[tokio::test]
async fn aip_211_authorization_non_leaking_denial() {
    use tonic_types::StatusExt as _;

    let (freight, iam) = make_server();

    // Create a shipper, then lock it to alice.
    let shipper = freight
        .create_shipper(Request::new(CreateShipperRequest {
            shipper: Some(Shipper {
                display_name: "Locked Corp".to_owned(),
                ..Default::default()
            }),
            request_id: String::new(),
            ..Default::default()
        }))
        .await
        .expect("create_shipper succeeds")
        .into_inner();

    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: shipper.name.clone(),
        policy: Some(policy_v1(
            "roles/freight.viewer",
            &["user:alice@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("lock shipper to alice");

    // alice reads it fine.
    let got = freight
        .get_shipper(as_caller(
            "user:alice@example.com",
            GetShipperRequest {
                name: shipper.name.clone(),
            },
        ))
        .await
        .expect("alice reads the locked shipper")
        .into_inner();
    assert_eq!(got, shipper);

    // bob and an anonymous caller get the canonical non-leaking
    // PERMISSION_DENIED whose message hides whether the resource exists.
    for caller in [Some("user:bob@example.com"), None] {
        let req: Request<GetShipperRequest> = match caller {
            Some(c) => as_caller(
                c,
                GetShipperRequest {
                    name: shipper.name.clone(),
                },
            ),
            None => Request::new(GetShipperRequest {
                name: shipper.name.clone(),
            }),
        };
        let status = freight
            .get_shipper(req)
            .await
            .expect_err("unauthorized caller is denied");
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            status.message(),
            format!(
                "Permission 'freight.shippers.get' denied on resource '{}' \
                 (or it might not exist).",
                shipper.name,
            ),
        );
        let info = status
            .get_details_error_info()
            .expect("ErrorInfo is attached");
        assert_eq!(info.reason, "IAM_PERMISSION_DENIED");
        assert_eq!(info.domain, "aip-rs");
    }

    // Non-leaking: bob cannot probe existence. Lock a never-created shipper and
    // the parent collection against alice-only, so bob is unauthorized on both.
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: "shippers/ghost".to_owned(),
        policy: Some(policy_v1(
            "roles/freight.viewer",
            &["user:alice@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("lock ghost");
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: "shippers".to_owned(),
        policy: Some(policy_v1(
            "roles/freight.viewer",
            &["user:alice@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("lock collection");

    // bob gets PERMISSION_DENIED on the missing name — same as on the locked
    // existing shipper, proving existence is not revealed.
    let on_missing = freight
        .get_shipper(as_caller(
            "user:bob@example.com",
            GetShipperRequest {
                name: "shippers/ghost".to_owned(),
            },
        ))
        .await
        .expect_err("bob is denied on the missing shipper");
    assert_eq!(on_missing.code(), tonic::Code::PermissionDenied);
    assert_eq!(
        on_missing
            .get_details_error_info()
            .expect("ErrorInfo")
            .reason,
        "IAM_PERMISSION_DENIED",
    );

    // AIP-211 fallback: grant bob on the parent collection. Now the missing name
    // comes back NOT_FOUND (he is allowed to learn the resource is absent).
    iam.set_iam_policy(Request::new(SetIamPolicyRequest {
        resource: "shippers".to_owned(),
        policy: Some(policy_v1(
            "roles/freight.viewer",
            &["user:alice@example.com", "user:bob@example.com"],
        )),
        update_mask: None,
    }))
    .await
    .expect("grant bob on the collection");

    let status = freight
        .get_shipper(as_caller(
            "user:bob@example.com",
            GetShipperRequest {
                name: "shippers/ghost".to_owned(),
            },
        ))
        .await
        .expect_err("missing resource is revealed to a parent-authorized caller");
    assert_eq!(status.code(), tonic::Code::NotFound);
    let info = status
        .get_details_error_info()
        .expect("ErrorInfo is attached");
    assert_eq!(info.reason, "IAM_RESOURCE_NOT_FOUND");
}
