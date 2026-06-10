//! The `FreightService` gRPC implementation — the demo's whole point.
//!
//! The Shipper standard methods (Get/List/Create/Update/Delete) are wired up as
//! the worked reference; every place an aip-rs primitive belongs is marked with
//! a `TODO(aip #N)` tied to its tracking issue, so the handlers tighten up as the
//! per-feature crates land. Site, Shipment, and the batch method return
//! `Unimplemented` until they follow the same pattern.

use std::cmp::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aip::fieldbehavior::FieldBehavior;
use aip::iam::{Member, Permission};
use aip::ordering::OrderByRequest;
use aip::pagination::{PageRequest, PageToken};
use aip::validation::Validator;
use prost::Message as _;
use prost_reflect::{Kind, ReflectMessage};
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::proto::einride::example::freight::v1::{
    freight_service_server::FreightService, BatchGetSitesRequest, BatchGetSitesResponse,
    CreateShipmentRequest, CreateShipperRequest, CreateSiteRequest, DeleteShipmentRequest,
    DeleteShipperRequest, DeleteSiteRequest, GetShipmentRequest, GetShipperRequest, GetSiteRequest,
    ListShipmentsRequest, ListShipmentsResponse, ListShippersRequest, ListShippersResponse,
    ListSitesRequest, ListSitesResponse, Shipment, ShipmentResourceName, Shipper,
    ShipperResourceName, Site, SiteResourceName, UpdateShipmentRequest, UpdateShipperRequest,
    UpdateSiteRequest,
};
use crate::storage::{PolicyStore, Storage};

/// The shipper collection ID — the root segment of every shipper resource name,
/// used as the collection-level IAM resource (the AIP-211 parent fallback). The
/// generated [`ShipperResourceName::PATTERN`] is the source of truth for the
/// segment (ADR-0011); a test guards the two against drifting apart.
const SHIPPERS_COLLECTION: &str = "shippers";

/// Default page size when a `ListShippers` request leaves `page_size` unset —
/// AIP-158 says the server picks an appropriate default.
const DEFAULT_PAGE_SIZE: usize = 50;

/// Upper bound on a single page, so a client can't pull the whole store in one
/// request — AIP-158 allows the server to return fewer results than requested.
const MAX_PAGE_SIZE: usize = 1000;

/// The Site field paths that `ListSites` accepts in an AIP-132 `order_by`. Used
/// as the allow-list for [`OrderBy::validate_for_paths`]; the nested `lat_lng.*`
/// paths exercise `.`-separated Subfield ordering.
const SORTABLE_SITE_PATHS: &[&str] = &[
    "name",
    "display_name",
    "create_time",
    "update_time",
    "lat_lng.latitude",
    "lat_lng.longitude",
];

/// Serves `FreightService` over an in-memory [`Storage`].
///
/// `policies` is the resource-name-keyed IAM [`PolicyStore`] shared with
/// [`IamServer`](crate::iam::IamServer): the handlers read it to make the AIP-211
/// authorization decision they gate on, so a Policy set via `SetIamPolicy` governs
/// who may read a resource (aip #67).
#[derive(Default)]
pub struct FreightServer {
    storage: Storage,
    policies: Arc<PolicyStore>,
}

impl FreightServer {
    /// A server backed by an empty store and its own empty policy store. The
    /// binary always shares a store via [`with_policies`](Self::with_policies), so
    /// this stand-alone constructor is only used by the handler tests.
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            storage: Storage::new(),
            policies: Arc::new(PolicyStore::new()),
        }
    }

    /// A server backed by an empty store and an existing, shared [`PolicyStore`] —
    /// the one [`IamServer`](crate::iam::IamServer) mutates, so IAM Policies govern
    /// freight authorization.
    pub fn with_policies(policies: Arc<PolicyStore>) -> Self {
        Self {
            storage: Storage::new(),
            policies,
        }
    }

    /// The example's AIP-211 authorization gate (step 1): may `caller` act on
    /// `resource`? A resource with **no Policy attached is public** in this demo —
    /// mirroring the open `ListShippers` — so existence is not secret until you
    /// lock it down with `SetIamPolicy`; once a Policy is attached, the caller must
    /// be named in one of its **Bindings** (directly, or via `allUsers` /
    /// `allAuthenticatedUsers`).
    ///
    /// This is deliberately a coarse *membership* check, not the full
    /// role→permission expansion and **Condition** evaluation that is the
    /// authorization **decision** — that lands behind the opt-in cel-backed `eval`
    /// adapter (#66/#68, ADR-0010). Issue #67 contributes the AIP-211 error
    /// *shape* this gate returns on a denial ([`aip::iam::authz`]), not the
    /// decision.
    fn authorized(&self, caller: Option<&Member>, resource: &str) -> bool {
        match self.policies.get(resource) {
            // Unprotected ⇒ public (a demo simplification, not production policy).
            None => true,
            Some(policy) => policy
                .bindings
                .iter()
                .flat_map(|binding| &binding.members)
                .any(|member| member_matches(member, caller)),
        }
    }
}

#[tonic::async_trait]
impl FreightService for FreightServer {
    // ----- Shipper: standard methods (the worked reference) -----

    async fn get_shipper(
        &self,
        request: Request<GetShipperRequest>,
    ) -> Result<Response<Shipper>, Status> {
        // AIP-211: authorize *before* validating or reading. The caller identity is
        // an example-owned credential carried in request metadata; a real server
        // derives the principal from authenticated transport instead.
        let caller = caller_member(request.metadata());
        let name = request.into_inner().name;

        if self.authorized(caller.as_ref(), &name) {
            // Authorized: a missing shipper is an honest `NOT_FOUND` (the caller is
            // allowed to know), and a malformed name is the usual `INVALID_ARGUMENT`.
            validate_shipper_name("name", &name)?;
            return self
                .storage
                .get_shipper(&name)
                .map(Response::new)
                .ok_or_else(|| Status::not_found(format!("shipper `{name}` not found")));
        }

        // Unauthorized: shape the AIP-211 error without leaking existence (#67).
        // The shipper exists ⇒ the canonical non-leaking `PERMISSION_DENIED`; it
        // does not ⇒ the `NOT_FOUND`-via-parent fallback, which reveals the gap
        // only to a caller that may read the parent collection's children and
        // otherwise returns the same `PERMISSION_DENIED`.
        let permission = shipper_get_permission();
        if self.storage.get_shipper(&name).is_some() {
            Err(aip::iam::authz::permission_denied(&permission, &name))
        } else {
            let parent_read = self.authorized(caller.as_ref(), SHIPPERS_COLLECTION);
            Err(aip::iam::authz::not_found_via_parent(
                &permission,
                &name,
                parent_read,
            ))
        }
    }

    async fn list_shippers(
        &self,
        request: Request<ListShippersRequest>,
    ) -> Result<Response<ListShippersResponse>, Status> {
        let req = request.into_inner();
        // Offset pagination (AIP-158) over the stable shipper listing. `parse_page`
        // checksums the request's non-pagination fields, verifies the offset token
        // against that checksum (rejecting a request that changed mid-pagination),
        // and resolves the page size; an empty token starts at offset 0.
        let page = parse_page(&req)?;
        let mut shippers = self.storage.list_shippers();
        let total = shippers.len();
        let start = usize::try_from(page.token.offset).unwrap_or(0).min(total);
        let end = start.saturating_add(page.size).min(total);
        // Only hand back a `next_page_token` when results remain past this page.
        let next_page_token = if end < total {
            page.token.next(page.size as i32).encode()
        } else {
            String::new()
        };
        let shippers = shippers.drain(start..end).collect();
        Ok(Response::new(ListShippersResponse {
            shippers,
            next_page_token,
        }))
    }

    async fn create_shipper(
        &self,
        request: Request<CreateShipperRequest>,
    ) -> Result<Response<Shipper>, Status> {
        let req = request.into_inner();
        // AIP-155 idempotency: a `request_id` makes the create safe to retry — a
        // replay with the same id returns the original shipper instead of minting
        // a second one. Fingerprint the request before its fields move out.
        let request_id = req.request_id.clone();
        let fingerprint = req.encode_to_vec();
        if let Some(existing) = idempotent_lookup::<_, Shipper>(&self.storage, &request_id, &req)? {
            return Ok(Response::new(existing));
        }
        let mut shipper = req
            .shipper
            .ok_or_else(|| Status::invalid_argument("shipper is required"))?;
        // Ignore any OUTPUT_ONLY or IMMUTABLE values the client sent (AIP-161).
        aip::fieldbehavior::clear_fields(
            &mut shipper,
            &[FieldBehavior::OutputOnly, FieldBehavior::Immutable],
        );
        // Validate that all REQUIRED fields are populated (AIP-203).
        aip::fieldbehavior::validate_required(&shipper).map_err(Status::from)?;
        // Mint a system-assigned resource ID (a UUIDv4) per AIP-148.
        // `CreateShipperRequest` has no `shipper_id` field, so there is no
        // user-supplied id to validate here; `validate_user_settable` guards
        // that path wherever a request later exposes one.
        let id = aip::resourceid::generate_system();
        // Format the canonical resource name `shippers/{shipper}` through the
        // typed wrapper generated from shipper.proto's `google.api.resource`
        // annotation (AIP-122 / ADR-0011) rather than hand-writing the pattern.
        shipper.name = ShipperResourceName { shipper: id }
            .format()
            .expect("a generated shipper id formats into the pattern");
        let ts = now();
        shipper.create_time = Some(ts);
        shipper.update_time = Some(ts);
        shipper.delete_time = None;
        // Stamp the AIP-154 content etag the client will echo back on a later
        // update/delete (#93). `compute_etag` digests the content — it ignores the
        // OUTPUT_ONLY timestamps just stamped and the etag field itself — so the
        // token tracks name/display_name, not server churn.
        shipper.etag = aip::etag::compute_etag(&shipper);
        // AIP-163: a `validate_only` request previews the would-be shipper —
        // system-assigned id and etag minted — without persisting it or recording
        // an idempotency entry, so a later real create still mints a new shipper.
        commit_unless_preview(req.validate_only, || {
            self.storage.put_shipper(shipper.clone());
            // Record the result so a retry carrying the same `request_id` replays it.
            idempotent_record(&self.storage, &request_id, fingerprint, &shipper);
        });
        Ok(Response::new(shipper))
    }

    async fn update_shipper(
        &self,
        request: Request<UpdateShipperRequest>,
    ) -> Result<Response<Shipper>, Status> {
        let req = request.into_inner();
        let incoming = req
            .shipper
            .ok_or_else(|| Status::invalid_argument("shipper is required"))?;
        let existing = self
            .storage
            .get_shipper(&incoming.name)
            .ok_or_else(|| Status::not_found(format!("shipper `{}` not found", incoming.name)))?;

        // AIP-154 freshness check (#93): an Update piggybacks the etag on the
        // resource, so the client's `shipper.etag` is the token it read. Verify it
        // against the stored shipper *before* doing any work — a stale etag means a
        // concurrent writer intervened (`ABORTED`, re-read and retry), a malformed
        // one is `INVALID_ARGUMENT`; both convert via the crate's AIP-193
        // `From<Error> for Status`. An empty etag opts out (an unconditional write).
        aip::etag::check_etag(&incoming.etag, &existing)?;

        // Apply the AIP-134 update mask via the field-mask primitive. The mask is
        // validated against the `Shipper` descriptor — sourced from the type
        // itself, since a Typed message carries its own (ADR-0009) — then the
        // request's shipper is merged into the stored one: an empty mask copies
        // the populated fields, `*` is a full replacement, and a named path absent
        // from the request clears that field.
        let mask = req.update_mask.unwrap_or_default();
        // An invalid mask path converts via the crate's AIP-193 `From<Error> for
        // Status` (#16): the client gets `INVALID_ARGUMENT` with an `ErrorInfo`
        // and, for a bad path, a `BadRequest` naming it.
        aip::fieldmask::validate(&mask, &Shipper::default().descriptor())?;
        // The typed `update` facade applies the mask straight on concrete
        // `Shipper`s — `existing` is the destination, `incoming` the source — and
        // transcodes through the dynamic core internally (ADR-0009), so the
        // handler never builds a `DynamicMessage`.
        let mut shipper = existing.clone();
        aip::fieldmask::update(&mask, &mut shipper, &incoming)?;

        // An update must not blank a REQUIRED field it names (AIP-203). Validate
        // only the REQUIRED fields whose exact path is in the mask: an empty mask
        // is a no-op (nothing on the wire can blank a field), and a field the mask
        // does not name keeps its stored value.
        aip::fieldbehavior::validate_required_with_mask(&shipper, &mask).map_err(Status::from)?;

        // Restore all OUTPUT_ONLY fields from the stored record — the client must
        // not move server-owned timestamps (AIP-161 / AIP-203).
        aip::fieldbehavior::copy_fields(&mut shipper, &existing, &[FieldBehavior::OutputOnly]);
        // Stamp the server-controlled update_time regardless of what was copied.
        shipper.update_time = Some(now());
        // Recompute the AIP-154 etag over the updated content (#93). `compute_etag`
        // ignores the etag field, so whatever the mask copied into it is replaced
        // by the fresh token; the response carries the value the next read-modify-
        // write must echo.
        shipper.etag = aip::etag::compute_etag(&shipper);
        // AIP-163: a `validate_only` request previews the merged shipper without
        // persisting it, so the stored shipper is left untouched.
        commit_unless_preview(req.validate_only, || {
            self.storage.put_shipper(shipper.clone())
        });
        Ok(Response::new(shipper))
    }

    async fn delete_shipper(
        &self,
        request: Request<DeleteShipperRequest>,
    ) -> Result<Response<Shipper>, Status> {
        let req = request.into_inner();
        // Soft delete (AIP-164) is deferred; this is a hard delete.
        validate_shipper_name("name", &req.name)?;
        // AIP-154 freshness check (#93): a Delete can't piggyback the etag on the
        // resource, so it rides on the request. Look up the shipper (a missing one
        // is `NOT_FOUND`, which takes precedence) and verify the supplied etag — a
        // stale token is `ABORTED`, a malformed one `INVALID_ARGUMENT` (AIP-193); an
        // empty etag makes the delete unconditional.
        let existing = self
            .storage
            .get_shipper(&req.name)
            .ok_or_else(|| Status::not_found(format!("shipper `{}` not found", req.name)))?;
        aip::etag::check_etag(&req.etag, &existing)?;
        // Existence was just established, so the removal returns the same record;
        // hand `existing` back as the deleted resource.
        self.storage.remove_shipper(&req.name);
        Ok(Response::new(existing))
    }

    // ----- Site: not yet wired (will mirror the Shipper handlers) -----

    async fn get_site(&self, _: Request<GetSiteRequest>) -> Result<Response<Site>, Status> {
        Err(unimplemented("GetSite"))
    }

    async fn list_sites(
        &self,
        request: Request<ListSitesRequest>,
    ) -> Result<Response<ListSitesResponse>, Status> {
        let req = request.into_inner();
        validate_shipper_name("parent", &req.parent)?;

        // Parse and validate the AIP-132 `order_by` against the allow-list of
        // sortable Site paths (#9's `validate_for_paths`, not the descriptor-based
        // `validate_for_message` of #10). Bad syntax or an unknown ordering field
        // converts via the crate's AIP-193 `From<Error> for Status` (#16) to
        // `INVALID_ARGUMENT` with an `ErrorInfo`, plus a `BadRequest` naming the
        // offending field path.
        let order_by = aip::ordering::parse_order_by(&req)?;
        order_by.validate_for_paths(SORTABLE_SITE_PATHS)?;

        // Offset pagination (AIP-158). `order_by` is a non-pagination field, so
        // the request checksum `parse_page` computes covers it: changing it
        // mid-pagination flips the checksum and the now-stale token is rejected.
        let page = parse_page(&req)?;

        // The AIP-160 `filter` is parsed + type-checked (`aip::filtering`) and
        // transpiled to a parameterized `Predicate` (`aip::sql`); an empty filter
        // adds nothing. The server then folds in its own predicates — the AIP
        // parent scope and the soft-delete `delete_time IS NULL` — through
        // `Predicate`, which owns precedence and one coherent placeholder
        // numbering across every composed fragment, so a user `a OR b` can't
        // silently re-associate against the server's `AND`s (#43). The SQLite
        // store renders the whole thing to one parameterized `WHERE`.
        let user_filter = parse_filter(&req.filter, &site_declarations(), &site_schema())?;
        let predicate = scoped_predicate(&req.parent, user_filter);

        // Sort and page in SQL (#42). The validated `order_by` transpiles to SQL
        // `ORDER BY` items, mapped through the same column `Schema` the filter
        // uses; the resource name is appended as a tie-break so the order is total
        // and stable across pages — equal `order_by` keys fall back to a fixed
        // name order. Every sortable path is in the allow-list and the schema maps
        // it, so transpilation can only fail on an allow-list/schema drift, an
        // internal inconsistency rather than bad input.
        let mut order = aip::sql::transpile_order_by(&order_by, &site_schema())
            .map_err(|e| Status::internal(format!("transpile order_by: {e}")))?;
        order.push(aip::sql::Order::asc("name"));

        // Fetch one row past the page so an extra row signals a further page (the
        // AIP-158 `next_page_token`). The parent scope now lives in the SQL
        // `WHERE` (#43), so the `LIMIT`/`OFFSET` page boundaries are computed over
        // exactly the in-scope rows — no in-memory post-filter that could
        // under-fill a page. A forged token's offset is clamped non-negative.
        let offset = page.token.offset.max(0) as u64;
        let mut sites =
            self.storage
                .list_sites_page(&predicate, &order, page.size as u64 + 1, offset);
        let has_more = sites.len() > page.size;
        sites.truncate(page.size);

        let next_page_token = if has_more {
            page.token.next(page.size as i32).encode()
        } else {
            String::new()
        };
        Ok(Response::new(ListSitesResponse {
            sites,
            next_page_token,
        }))
    }

    async fn create_site(
        &self,
        request: Request<CreateSiteRequest>,
    ) -> Result<Response<Site>, Status> {
        let req = request.into_inner();
        // AIP-155 idempotency, as in `create_shipper`: a `request_id` replay
        // returns the original site rather than minting a second one.
        let request_id = req.request_id.clone();
        let fingerprint = req.encode_to_vec();
        if let Some(existing) = idempotent_lookup::<_, Site>(&self.storage, &request_id, &req)? {
            return Ok(Response::new(existing));
        }
        validate_shipper_name("parent", &req.parent)?;
        let mut site = req
            .site
            .ok_or_else(|| Status::invalid_argument("site is required"))?;
        // Accumulate the server's own presence checks (no aip-rs primitive covers
        // them) into one AIP-193 error, so the client sees every bad field at once.
        let mut violations = Validator::new(SERVICE_DOMAIN, "FIELD_REQUIRED");
        if site.display_name.is_empty() {
            violations.add_field_violation("site.display_name", "field is required");
        }
        violations.into_result()?;

        // The validated `parent` binds the `{shipper}` of the canonical site
        // pattern; mint a system-assigned `{site}` id (a UUIDv4, per AIP-148) and
        // format the full resource name through the typed wrappers generated from
        // the `google.api.resource` annotations (AIP-122 / ADR-0011).
        let parent = ShipperResourceName::parse(&req.parent)
            .expect("parent validated to match the shipper pattern");
        site.name = SiteResourceName {
            shipper: parent.shipper,
            site: aip::resourceid::generate_system(),
        }
        .format()
        .expect("a generated site id formats into the pattern");

        let ts = now();
        site.create_time = Some(ts);
        site.update_time = Some(ts);
        site.delete_time = None;
        // AIP-163: a `validate_only` request previews the would-be site without
        // persisting it or recording an idempotency entry.
        commit_unless_preview(req.validate_only, || {
            self.storage.put_site(site.clone());
            idempotent_record(&self.storage, &request_id, fingerprint, &site);
        });
        Ok(Response::new(site))
    }

    async fn update_site(&self, _: Request<UpdateSiteRequest>) -> Result<Response<Site>, Status> {
        Err(unimplemented("UpdateSite"))
    }

    async fn delete_site(&self, _: Request<DeleteSiteRequest>) -> Result<Response<Site>, Status> {
        Err(unimplemented("DeleteSite"))
    }

    async fn batch_get_sites(
        &self,
        _: Request<BatchGetSitesRequest>,
    ) -> Result<Response<BatchGetSitesResponse>, Status> {
        Err(unimplemented("BatchGetSites"))
    }

    // ----- Shipment: not yet wired -----

    async fn get_shipment(
        &self,
        _: Request<GetShipmentRequest>,
    ) -> Result<Response<Shipment>, Status> {
        Err(unimplemented("GetShipment"))
    }

    async fn list_shipments(
        &self,
        request: Request<ListShipmentsRequest>,
    ) -> Result<Response<ListShipmentsResponse>, Status> {
        let req = request.into_inner();
        validate_shipper_name("parent", &req.parent)?;

        // Offset pagination (AIP-158). `filter` is a non-pagination field, so the
        // request checksum `parse_page` computes covers it: changing it
        // mid-pagination flips the checksum and the stale token is rejected.
        let page = parse_page(&req)?;

        // The same server-side composition as `ListSites` (#43): the user's
        // AIP-160 `filter` (parsed + type-checked + transpiled) folded with the
        // parent scope and the soft-delete `delete_time IS NULL` through one
        // `Predicate` that owns precedence and placeholder numbering. The
        // SQLite-backed store renders it to a parameterized `WHERE`.
        let user_filter = parse_filter(&req.filter, &shipment_declarations(), &shipment_schema())?;
        let predicate = scoped_predicate(&req.parent, user_filter);

        // `ListShipments` carries no `order_by`, so results are ordered by
        // resource name — a total, stable page order across the offset pages.
        let order = [aip::sql::Order::asc("name")];
        let offset = page.token.offset.max(0) as u64;
        let mut shipments =
            self.storage
                .list_shipments_page(&predicate, &order, page.size as u64 + 1, offset);
        let has_more = shipments.len() > page.size;
        shipments.truncate(page.size);

        let next_page_token = if has_more {
            page.token.next(page.size as i32).encode()
        } else {
            String::new()
        };
        Ok(Response::new(ListShipmentsResponse {
            shipments,
            next_page_token,
        }))
    }

    async fn create_shipment(
        &self,
        request: Request<CreateShipmentRequest>,
    ) -> Result<Response<Shipment>, Status> {
        // Mirrors `create_site`: the only shipment write the demo needs, so
        // `ListShipments` (#43) has something to filter and page. The other
        // shipment standard methods stay `Unimplemented` until their issues land.
        let req = request.into_inner();
        // AIP-155 idempotency, as in `create_shipper`: a `request_id` replay
        // returns the original shipment rather than minting a second one.
        let request_id = req.request_id.clone();
        let fingerprint = req.encode_to_vec();
        if let Some(existing) = idempotent_lookup::<_, Shipment>(&self.storage, &request_id, &req)?
        {
            return Ok(Response::new(existing));
        }
        validate_shipper_name("parent", &req.parent)?;
        let mut shipment = req
            .shipment
            .ok_or_else(|| Status::invalid_argument("shipment is required"))?;
        // Both endpoints are required (AIP-203). Accumulate them into one
        // `Validator` so a request missing both gets both violations back in a
        // single response, rather than bailing on the first.
        let mut violations = Validator::new(SERVICE_DOMAIN, "FIELD_REQUIRED");
        if shipment.origin_site.is_empty() {
            violations.add_field_violation("shipment.origin_site", "field is required");
        }
        if shipment.destination_site.is_empty() {
            violations.add_field_violation("shipment.destination_site", "field is required");
        }
        violations.into_result()?;

        // The validated `parent` binds the `{shipper}` of the canonical shipment
        // pattern; mint a system-assigned `{shipment}` id (a UUIDv4, per AIP-148)
        // and format the full resource name through the typed wrappers generated
        // from the `google.api.resource` annotations (AIP-122 / ADR-0011).
        let parent = ShipperResourceName::parse(&req.parent)
            .expect("parent validated to match the shipper pattern");
        shipment.name = ShipmentResourceName {
            shipper: parent.shipper,
            shipment: aip::resourceid::generate_system(),
        }
        .format()
        .expect("a generated shipment id formats into the pattern");

        let ts = now();
        shipment.create_time = Some(ts);
        shipment.update_time = Some(ts);
        shipment.delete_time = None;
        // AIP-163: a `validate_only` request previews the would-be shipment
        // without persisting it or recording an idempotency entry.
        commit_unless_preview(req.validate_only, || {
            self.storage.put_shipment(shipment.clone());
            idempotent_record(&self.storage, &request_id, fingerprint, &shipment);
        });
        Ok(Response::new(shipment))
    }

    async fn update_shipment(
        &self,
        _: Request<UpdateShipmentRequest>,
    ) -> Result<Response<Shipment>, Status> {
        Err(unimplemented("UpdateShipment"))
    }

    async fn delete_shipment(
        &self,
        _: Request<DeleteShipmentRequest>,
    ) -> Result<Response<Shipment>, Status> {
        Err(unimplemented("DeleteShipment"))
    }
}

/// Lets `aip::pagination` read the AIP-158 pagination fields off the generated
/// request without reflection.
impl PageRequest for ListShippersRequest {
    fn page_token(&self) -> &str {
        &self.page_token
    }
    fn page_size(&self) -> i32 {
        self.page_size
    }
}

/// `ListSitesRequest` carries the full pagination field set, including AIP-158
/// `skip` (which `ListShippersRequest` omits).
impl PageRequest for ListSitesRequest {
    fn page_token(&self) -> &str {
        &self.page_token
    }
    fn page_size(&self) -> i32 {
        self.page_size
    }
    fn skip(&self) -> i32 {
        self.skip
    }
}

/// `ListShipmentsRequest` carries the pagination fields but no AIP-158 `skip`.
impl PageRequest for ListShipmentsRequest {
    fn page_token(&self) -> &str {
        &self.page_token
    }
    fn page_size(&self) -> i32 {
        self.page_size
    }
}

/// Lets `aip::ordering::parse_order_by` read the AIP-132 `order_by` field off
/// the generated request.
impl OrderByRequest for ListSitesRequest {
    fn order_by(&self) -> &str {
        &self.order_by
    }
}

// The AIP-132 sort and AIP-158 page are pushed to SQLite (#42): `list_sites`
// transpiles `order_by` to SQL `ORDER BY` items and the store renders `ORDER BY`
// / `LIMIT` / `OFFSET`, so the in-memory `sort_sites` the earlier slices used is
// gone.

/// The AIP-160 declarations a `ListSites` filter is checked against: the full
/// standard operator set (#40, #41) over one identifier of each filterable
/// shape — the string `display_name` / `name`, the timestamp `create_time`, the
/// nested numeric `lat_lng.latitude`, the reflective enum `state`, the
/// `annotations` map, and the `tags` list. The map / list / string / timestamp
/// identifiers carry the has operator `:` overloads (#41).
fn site_declarations() -> aip::filtering::Declarations {
    use aip::filtering::{Declarations, Type};

    // The `Site.state` enum descriptor, taken from the field itself — a Typed
    // message carries its own descriptor (ADR-0009), so there is no by-name pool
    // lookup. `enum_ident` declares the field, each value name, and the enum
    // `=`/`!=` overloads.
    let state_enum = match Site::default()
        .descriptor()
        .get_field_by_name("state")
        .expect("Site has a `state` field")
        .kind()
    {
        Kind::Enum(descriptor) => descriptor,
        other => unreachable!("the `state` field is an enum, found {other:?}"),
    };

    Declarations::builder()
        .standard_functions()
        .ident("display_name", Type::String)
        .ident("name", Type::String)
        .ident("create_time", Type::Timestamp)
        .ident("lat_lng.latitude", Type::Double)
        .ident(
            "annotations",
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
        )
        .ident("tags", Type::List(Box::new(Type::String)))
        .enum_ident("state", state_enum)
        .build()
        .expect("site filter declarations are valid")
}

/// Maps the Site identifiers a filter or `order_by` can address onto their SQLite
/// columns (#39, #40, #41, #42). The nested `lat_lng.latitude` / `lat_lng.longitude`
/// paths flatten to the `latitude` / `longitude` columns; `annotations` / `tags`
/// are the JSON map / list columns the has operator queries with `json_each`; the
/// rest map to identically-named columns in the `sites` table.
///
/// This is the column allowlist for both [`aip::sql::transpile_filter`] and
/// [`aip::sql::transpile_order_by`], so it must cover every sortable
/// [`SORTABLE_SITE_PATHS`] entry (the `update_time` / `lat_lng.longitude` columns
/// are sort-only — they carry no filter [`Declaration`](aip::filtering)).
fn site_schema() -> aip::sql::Schema {
    aip::sql::Schema::builder()
        .column("display_name", "display_name")
        .column("name", "name")
        .column("create_time", "create_time")
        .column("update_time", "update_time")
        .column("lat_lng.latitude", "latitude")
        .column("lat_lng.longitude", "longitude")
        .column("state", "state")
        .column("annotations", "annotations")
        .column("tags", "tags")
        .build()
}

/// The AIP-160 declarations a `ListShipments` filter is checked against: the
/// resource-name references `origin_site` / `destination_site` (strings, so they
/// also carry the substring has operator `:`), the timestamp `create_time`, and
/// the `annotations` map (carrying the key-presence has operator). A small, focused
/// allowlist — `ListShipments` exists here to prove the *composition* (#43), not to
/// re-enumerate every filterable shape `ListSites` already covers.
fn shipment_declarations() -> aip::filtering::Declarations {
    use aip::filtering::{Declarations, Type};

    Declarations::builder()
        .standard_functions()
        .ident("name", Type::String)
        .ident("origin_site", Type::String)
        .ident("destination_site", Type::String)
        .ident("create_time", Type::Timestamp)
        .ident(
            "annotations",
            Type::Map(Box::new(Type::String), Box::new(Type::String)),
        )
        .build()
        .expect("shipment filter declarations are valid")
}

/// Maps the Shipment identifiers a filter can address onto their SQLite columns
/// (#43) in the `shipments` table; `annotations` is the JSON map column the has
/// operator queries with `json_each`. This is the column allowlist for
/// [`aip::sql::transpile_filter`], paired with [`shipment_declarations`].
fn shipment_schema() -> aip::sql::Schema {
    aip::sql::Schema::builder()
        .column("name", "name")
        .column("origin_site", "origin_site")
        .column("destination_site", "destination_site")
        .column("create_time", "create_time")
        .column("annotations", "annotations")
        .build()
}

/// Parse + type-check an AIP-160 `filter` and transpile it to a parameterized
/// [`Predicate`](aip::sql::Predicate), or `Ok(None)` for an empty filter (which
/// lists every in-scope row). Shared by `ListSites` and `ListShipments` (#43).
///
/// An invalid filter converts to `INVALID_ARGUMENT`: `check` via `aip-filtering`'s
/// AIP-193 `From<Error>` (#16), and any construct the transpiler can't lower (e.g.
/// a comparison between two columns) explicitly. The same `declarations` drive the
/// check and the transpiler's type recovery — it recovers enum/timestamp/map/list
/// typing from them (ADR-0008).
fn parse_filter(
    filter: &str,
    declarations: &aip::filtering::Declarations,
    schema: &aip::sql::Schema,
) -> Result<Option<aip::sql::Predicate>, Status> {
    if filter.is_empty() {
        return Ok(None);
    }
    let checked = aip::filtering::check(filter, declarations)?;
    let predicate = aip::sql::transpile_filter(&checked, declarations, schema)
        .map_err(|e| Status::invalid_argument(format!("filter: {e}")))?;
    Ok(Some(predicate))
}

/// Compose the server's own predicates with an optional user `filter` into one
/// [`Predicate`](aip::sql::Predicate) (#43): an AIP parent scope on the `name`
/// column (`name LIKE 'parent/%'`, the parent escaped + bound) and the soft-delete
/// `delete_time IS NULL`, conjoined with the user filter when present. `Predicate`
/// owns precedence and one coherent placeholder numbering across the fragments, so
/// a user `a OR b` is parenthesized under the server's `AND`s rather than silently
/// re-associating, and the bound parent never collides with the filter's binds.
///
/// A multi-tenant server adds its tenancy predicate to the very same `all` — e.g.
/// `aip::sql::Predicate::eq("tenant_id", tenant)` — and it numbers in step with
/// the rest; here the parent scope is the freight demo's tenancy boundary (a
/// shipper owns its sites and shipments).
fn scoped_predicate(parent: &str, user_filter: Option<aip::sql::Predicate>) -> aip::sql::Predicate {
    let mut clauses = vec![
        aip::sql::Predicate::scope_to_parent("name", parent),
        aip::sql::Predicate::is_null("delete_time"),
    ];
    // `Option` is an iterator of 0-or-1, so this appends the user filter only when
    // one was supplied.
    clauses.extend(user_filter);
    aip::sql::Predicate::all(clauses)
}

/// The resolved AIP-158 pagination state for one list page, produced by
/// [`parse_page`]: the verified offset page token and the clamped page size.
struct Page {
    /// The verified offset page token. `token.offset` is where this page starts;
    /// `token.next(size)` mints the following page's token, carrying the request
    /// checksum forward so a mid-pagination change is still rejected.
    token: PageToken,
    /// The page size after the AIP-158 default/cap has been applied.
    size: usize,
}

/// Folds the AIP-158 list-pagination preamble into a single step: checksum the
/// request's non-pagination fields, parse and verify the offset page token
/// against that checksum, and resolve the page size. Both list handlers open
/// their pagination logic with `parse_page(&req)?`.
fn parse_page<M: PageRequest + ReflectMessage>(request: &M) -> Result<Page, Status> {
    // Compute the request checksum directly off the concrete request. Since #46
    // the generated types are Typed messages (`ReflectMessage`), so the descriptor
    // travels with the value and `request_checksum` takes it without the
    // `DynamicMessage` bridge or a hand-derived message name (ADR-0009). A
    // checksum failure would mean the type and its descriptor disagree — a build
    // bug, not bad input — so it surfaces as `internal`.
    let checksum = aip::pagination::request_checksum(request)
        .map_err(|e| Status::internal(format!("compute request checksum: {e}")))?;
    // A malformed token, version mismatch, or checksum mismatch converts via the
    // crate's AIP-193 `From<Error> for Status` (#16) to an `INVALID_ARGUMENT`
    // carrying an `ErrorInfo` (e.g. `PAGE_TOKEN_CHECKSUM_MISMATCH`).
    let token = PageToken::parse(request, checksum)?;
    let size = effective_page_size(request.page_size())?;
    Ok(Page { token, size })
}

/// Resolves the effective page size from a request's `page_size` per AIP-158: a
/// negative value is rejected with `INVALID_ARGUMENT`, zero/unset falls back to
/// [`DEFAULT_PAGE_SIZE`], and a positive value is capped at [`MAX_PAGE_SIZE`] (the
/// server may return fewer results than the client requested).
fn effective_page_size(requested: i32) -> Result<usize, Status> {
    match requested.cmp(&0) {
        Ordering::Less => Err(Status::invalid_argument("page_size must not be negative")),
        Ordering::Equal => Ok(DEFAULT_PAGE_SIZE),
        Ordering::Greater => Ok((requested as usize).min(MAX_PAGE_SIZE)),
    }
}

/// Validates that `value` is a well-formed shipper resource name (AIP-122): a
/// valid resource name that parses as a [`ShipperResourceName`] — the typed
/// wrapper generated from shipper.proto's `google.api.resource` pattern. Returns
/// `INVALID_ARGUMENT` otherwise. `field` is the request field the value came from
/// (`name` or `parent`), so the AIP-193 `BadRequest` points at the right one.
fn validate_shipper_name(field: &str, value: &str) -> Result<(), Status> {
    // A malformed name converts via the crate's AIP-193 `From<Error> for Status`
    // (#16) to an `INVALID_ARGUMENT` carrying an `ErrorInfo`. The pattern-match
    // check below is the server's own policy, so it builds its own AIP-193 details.
    aip::resourcename::validate(value)?;
    if ShipperResourceName::parse(value).is_err() {
        let mut violations = Validator::new(SERVICE_DOMAIN, "RESOURCE_NAME_PATTERN_MISMATCH");
        violations.add_field_violation(
            field,
            format!("must match the pattern `{}`", ShipperResourceName::PATTERN),
        );
        violations.into_result()?;
    }
    Ok(())
}

/// The AIP-193 `ErrorInfo.domain` for errors the server raises itself — the
/// presence and policy checks no aip-rs primitive covers. Passed to every
/// [`Validator`] the handlers build, so the service's own field violations carry
/// its domain; the aip-rs crates use their own (`aip-rs`) domain for the values
/// they validate (#16).
const SERVICE_DOMAIN: &str = "freight.example.com";

/// The request-metadata key the demo reads the caller's IAM **Member** identity
/// from. A real server derives the principal from authenticated transport (mTLS, a
/// verified JWT); the demo takes it verbatim so `grpcurl -H 'x-freight-caller: …'`
/// can play any identity against the AIP-211 gate (and `TestIamPermissions`, #68).
pub(crate) const CALLER_METADATA_KEY: &str = "x-freight-caller";

/// Read the caller's IAM **Member** from request metadata, or `None` when it is
/// absent or unparseable (an anonymous caller). The credential only *identifies*
/// the caller for the authorization gate — it is not a request field — so a bad
/// value degrades to anonymous rather than `INVALID_ARGUMENT`. Shared with the
/// `IAMPolicy` service's `TestIamPermissions` (#68), which reads the same caller.
pub(crate) fn caller_member(metadata: &MetadataMap) -> Option<Member> {
    metadata
        .get(CALLER_METADATA_KEY)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Member>().ok())
}

/// Does the stored Policy member string `granted` admit `caller`? `allUsers`
/// admits anyone (even an absent caller); `allAuthenticatedUsers` admits any
/// present caller; a typed member admits the exact same **Member**. The grant is
/// compared against the caller's canonical [`Member`] rendering, so only a
/// well-formed grant matches (a malformed one was rejected at `SetIamPolicy`).
/// Shared with `TestIamPermissions` (#68), which matches the caller the same way.
pub(crate) fn member_matches(granted: &str, caller: Option<&Member>) -> bool {
    match granted {
        "allUsers" => true,
        "allAuthenticatedUsers" => caller.is_some(),
        granted => caller.is_some_and(|member| member.to_string() == granted),
    }
}

/// The AIP-211 permission `GetShipper` checks (`freight.shippers.get`) — the
/// `Permission` named in a denial's non-leaking message and `IAM_*` `ErrorInfo`.
fn shipper_get_permission() -> Permission {
    "freight.shippers.get"
        .parse()
        .expect("a static freight.shippers.get permission is well-formed")
}

/// AIP-155 idempotency pre-check for a create handler (issue #94).
///
/// Validates `request_id` (a malformed id is an AIP-193 `INVALID_ARGUMENT`) and
/// resolves it against the server's cache of seen ids:
/// - empty id ⇒ no idempotency requested, returns `Ok(None)` (proceed);
/// - unseen id ⇒ `Ok(None)` (proceed, then [`idempotent_record`] the result);
/// - same id + identical request ⇒ `Ok(Some(response))`, the recorded response
///   decoded back into `Resp` — a safe retry replays rather than re-creates;
/// - same id + different request ⇒ `Err` with the `REQUEST_ID_CONFLICT` status.
///
/// The recorded request is compared to the incoming one **structurally** (prost
/// `PartialEq`), not byte-for-byte: a proto `map` field is a `HashMap` whose wire
/// order is non-deterministic across encodes, so two identical requests can
/// serialize to different bytes. Structural equality is order-independent, so a
/// safe retry — even one carrying an `annotations` map — stays a Replayed rather
/// than a false Conflict. The library names the
/// [`Replay`](aip::requestid::Replay) outcomes; this server owns the storage and
/// the comparison.
fn idempotent_lookup<Req, Resp>(
    storage: &Storage,
    request_id: &str,
    request: &Req,
) -> Result<Option<Resp>, Status>
where
    Req: prost::Message + Default + PartialEq,
    Resp: prost::Message + Default,
{
    if request_id.is_empty() {
        return Ok(None);
    }
    aip::requestid::validate(request_id).map_err(Status::from)?;
    let recorded = storage.idempotent_get(request_id);
    let matches = recorded.as_ref().map(|record| {
        Req::decode(record.request.as_slice())
            .map(|stored| &stored == request)
            .unwrap_or(false)
    });
    match aip::requestid::Replay::decide(matches) {
        aip::requestid::Replay::New => Ok(None),
        aip::requestid::Replay::Replayed => {
            let record = recorded.expect("a recorded id ⇒ Replayed");
            Ok(Some(
                Resp::decode(record.response.as_slice()).expect("decode the recorded response"),
            ))
        }
        aip::requestid::Replay::Conflict => Err(aip::requestid::conflict(request_id)),
    }
}

/// Record a create's request + response under its `request_id`, so a later retry
/// replays through [`idempotent_lookup`] (AIP-155, issue #94). A no-op for an
/// empty id (no idempotency was requested).
fn idempotent_record(
    storage: &Storage,
    request_id: &str,
    fingerprint: Vec<u8>,
    response: &impl prost::Message,
) {
    if request_id.is_empty() {
        return;
    }
    storage.idempotent_put(request_id.to_owned(), fingerprint, response.encode_to_vec());
}

/// AIP-163 preview gate: persist a mutation only when the request is a real
/// write. A `validate_only` request has already run the full validation pipeline
/// above (REQUIRED fields, the [`Validator`] accumulator, resource-name and etag
/// checks) and built the would-be resource; this skips `commit`, so the preview
/// leaves the store — and the AIP-155 idempotency cache — untouched while the
/// handler still returns the resource it would have committed. Validation runs
/// unconditionally before this point, so a request that would fail fails
/// identically with or without the flag, and `validate_only` stays one line per
/// handler rather than a forked validation path.
fn commit_unless_preview(validate_only: bool, commit: impl FnOnce()) {
    if !validate_only {
        commit();
    }
}

/// The standard `Unimplemented` status for a method that hasn't been wired yet.
fn unimplemented(method: &str) -> Status {
    Status::unimplemented(format!(
        "{method} is not implemented yet in the aip-rs demo"
    ))
}

/// Current wall-clock time as a protobuf `Timestamp`, for the server-set
/// OUTPUT_ONLY `create_time`/`update_time` fields.
fn now() -> prost_types::Timestamp {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    prost_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::einride::example::freight::v1::site;
    use aip::ordering::OrderBy;
    use aip_proto::google::r#type::LatLng;

    /// A shipper parent name; the demo does not require the shipper to exist in
    /// storage for `CreateSite`/`ListSites`, only that the name is well-formed.
    const PARENT: &str = "shippers/acme";

    /// The proto's `google.api.resource` pattern is the source of truth for the
    /// shipper collection segment (ADR-0011): if the generated pattern ever
    /// changes, the hand-held collection handle the IAM gate consults must move
    /// with it.
    #[test]
    fn shippers_collection_matches_the_generated_pattern() {
        assert_eq!(
            ShipperResourceName::PATTERN.split_once('/'),
            Some((SHIPPERS_COLLECTION, "{shipper}")),
        );
    }

    /// Creates a site under `PARENT` with the given display name and latitude.
    async fn seed_site(server: &FreightServer, display_name: &str, latitude: f64) {
        let site = Site {
            display_name: display_name.to_owned(),
            lat_lng: Some(LatLng {
                latitude,
                longitude: 0.0,
            }),
            ..Default::default()
        };
        server
            .create_site(Request::new(CreateSiteRequest {
                parent: PARENT.to_owned(),
                site: Some(site),
                ..Default::default()
            }))
            .await
            .expect("create_site succeeds");
    }

    /// Creates a site under `PARENT` with the given display name and operational
    /// state, for the enum-filter test.
    async fn seed_site_with_state(server: &FreightServer, display_name: &str, state: site::State) {
        server
            .create_site(Request::new(CreateSiteRequest {
                parent: PARENT.to_owned(),
                site: Some(Site {
                    display_name: display_name.to_owned(),
                    state: state as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("create_site succeeds");
    }

    /// Creates a site under `PARENT` with the given display name, annotations map,
    /// and tags list, for the has-operator filter tests.
    async fn seed_site_with_metadata(
        server: &FreightServer,
        display_name: &str,
        annotations: &[(&str, &str)],
        tags: &[&str],
    ) {
        server
            .create_site(Request::new(CreateSiteRequest {
                parent: PARENT.to_owned(),
                site: Some(Site {
                    display_name: display_name.to_owned(),
                    annotations: annotations
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    tags: tags.iter().map(|t| t.to_string()).collect(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("create_site succeeds");
    }

    /// Lists sites under `PARENT` with the given `order_by`, returning their
    /// display names in the order the server produced them.
    async fn list_display_names(server: &FreightServer, order_by: &str) -> Vec<String> {
        let resp = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                order_by: order_by.to_owned(),
                ..Default::default()
            }))
            .await
            .expect("list_sites succeeds")
            .into_inner();
        resp.sites.into_iter().map(|s| s.display_name).collect()
    }

    #[tokio::test]
    async fn orders_by_display_name_ascending_and_descending() {
        let server = FreightServer::new();
        for name in ["Bravo", "Alpha", "Charlie"] {
            seed_site(&server, name, 0.0).await;
        }
        assert_eq!(
            list_display_names(&server, "display_name").await,
            ["Alpha", "Bravo", "Charlie"],
        );
        assert_eq!(
            list_display_names(&server, "display_name desc").await,
            ["Charlie", "Bravo", "Alpha"],
        );
    }

    #[tokio::test]
    async fn orders_by_nested_subfield_path() {
        let server = FreightServer::new();
        seed_site(&server, "north", 60.0).await;
        seed_site(&server, "south", -30.0).await;
        seed_site(&server, "equator", 0.0).await;
        // `lat_lng.latitude` is a `.`-nested Subfield path.
        assert_eq!(
            list_display_names(&server, "lat_lng.latitude").await,
            ["south", "equator", "north"],
        );
        assert_eq!(
            list_display_names(&server, "lat_lng.latitude desc").await,
            ["north", "equator", "south"],
        );
    }

    #[tokio::test]
    async fn orders_by_multiple_fields_in_priority() {
        // A multi-field `order_by` sorts by the first field, then the second
        // within each tie: latitude ascending groups the two `lat 0` sites, and
        // `display_name` orders them within that group.
        let server = FreightServer::new();
        seed_site(&server, "Bravo", 0.0).await;
        seed_site(&server, "Alpha", 0.0).await;
        seed_site(&server, "Crest", 1.0).await;
        assert_eq!(
            list_display_names(&server, "lat_lng.latitude, display_name").await,
            ["Alpha", "Bravo", "Crest"],
        );
        // Reversing only the secondary field flips the `lat 0` group's order while
        // the latitude grouping stays ascending.
        assert_eq!(
            list_display_names(&server, "lat_lng.latitude, display_name desc").await,
            ["Bravo", "Alpha", "Crest"],
        );
    }

    #[tokio::test]
    async fn rejects_invalid_order_by_with_invalid_argument() {
        let server = FreightServer::new();
        // `foo/bar` is bad syntax, `display_name bogus` has a non-direction word,
        // and `unknown_field` is well-formed but not in the sortable allow-list.
        for bad in ["foo/bar", "display_name bogus", "unknown_field"] {
            let status = server
                .list_sites(Request::new(ListSitesRequest {
                    parent: PARENT.to_owned(),
                    order_by: bad.to_owned(),
                    ..Default::default()
                }))
                .await
                .expect_err("invalid order_by is rejected");
            assert_eq!(
                status.code(),
                tonic::Code::InvalidArgument,
                "order_by {bad:?} should be InvalidArgument",
            );
        }
    }

    #[tokio::test]
    async fn paginates_stably_and_guards_order_by_change() {
        let server = FreightServer::new();
        for name in ["d", "b", "e", "a", "c"] {
            seed_site(&server, name, 0.0).await;
        }

        // Page through (size 2) ordered by display_name; the concatenation across
        // pages is the full, stably-ordered listing.
        let mut collected = Vec::new();
        let mut page_token = String::new();
        loop {
            let resp = server
                .list_sites(Request::new(ListSitesRequest {
                    parent: PARENT.to_owned(),
                    order_by: "display_name".to_owned(),
                    page_size: 2,
                    page_token: page_token.clone(),
                    ..Default::default()
                }))
                .await
                .expect("list_sites page succeeds")
                .into_inner();
            collected.extend(resp.sites.into_iter().map(|s| s.display_name));
            page_token = resp.next_page_token;
            if page_token.is_empty() {
                break;
            }
        }
        assert_eq!(collected, ["a", "b", "c", "d", "e"]);

        // A token minted under one `order_by` is rejected when replayed under a
        // different one: `order_by` is a non-pagination field, so the request
        // checksum (#7) changes and the stale token is refused.
        let first = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                order_by: "display_name".to_owned(),
                page_size: 2,
                ..Default::default()
            }))
            .await
            .expect("first page succeeds")
            .into_inner();
        assert!(!first.next_page_token.is_empty());
        let status = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                order_by: "name".to_owned(),
                page_size: 2,
                page_token: first.next_page_token,
                ..Default::default()
            }))
            .await
            .expect_err("changing order_by mid-pagination invalidates the token");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn effective_page_size_applies_aip158_rules() {
        // AIP-158: a negative `page_size` is rejected, zero/unset falls back to
        // the default, a positive value passes through, and anything above the
        // cap is clamped to the max.
        assert_eq!(
            effective_page_size(-1)
                .expect_err("negative is rejected")
                .code(),
            tonic::Code::InvalidArgument,
        );
        assert_eq!(
            effective_page_size(0).expect("zero is the default"),
            DEFAULT_PAGE_SIZE
        );
        assert_eq!(
            effective_page_size(10).expect("positive passes through"),
            10
        );
        assert_eq!(
            effective_page_size(i32::MAX).expect("over-max is clamped"),
            MAX_PAGE_SIZE,
        );
    }

    #[tokio::test]
    async fn list_sites_rejects_negative_page_size() {
        // A negative `page_size` is InvalidArgument (AIP-158), not a silent
        // fall-back to the default page.
        let server = FreightServer::new();
        let status = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                page_size: -1,
                ..Default::default()
            }))
            .await
            .expect_err("negative page_size is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn list_shippers_rejects_negative_page_size() {
        // The shared `parse_page` preamble rejects a negative `page_size` for
        // `ListShippers` too — independent of whether any shippers exist.
        let server = FreightServer::new();
        let status = server
            .list_shippers(Request::new(ListShippersRequest {
                page_size: -1,
                ..Default::default()
            }))
            .await
            .expect_err("negative page_size is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn sortable_site_paths_resolve_on_the_site_descriptor() {
        // `ListSites` gates `order_by` with the curated `validate_for_paths`
        // allow-list (#9). `validate_for_message` (#10) guards the allow-list
        // itself: every sortable path must be a real `Site` field, so the
        // allow-list can't silently drift from the proto. The `Site` descriptor
        // comes straight off the Typed message (ADR-0009), no by-name pool lookup.
        let site = Site::default().descriptor();
        let order_by: OrderBy = SORTABLE_SITE_PATHS
            .join(",")
            .parse()
            .expect("the allow-list is valid order_by syntax");
        order_by
            .validate_for_message(&site)
            .expect("every sortable Site path resolves on the Site descriptor");
    }

    #[test]
    fn sortable_site_paths_map_to_columns_in_the_schema() {
        // Sorting now happens in SQL (#42): `transpile_order_by` maps each
        // `order_by` path to a column through `site_schema`. So the schema must
        // cover every sortable path — otherwise an in-allow-list `order_by` would
        // surface as an `internal` error. This guards the allow-list against
        // drifting from the column schema the same way the test above guards it
        // against the proto.
        let order_by: OrderBy = SORTABLE_SITE_PATHS
            .join(",")
            .parse()
            .expect("the allow-list is valid order_by syntax");
        aip::sql::transpile_order_by(&order_by, &site_schema())
            .expect("every sortable Site path maps to a column in the schema");
    }

    #[test]
    fn validate_for_message_rejects_unknown_site_path() {
        let site = Site::default().descriptor();
        let order_by: OrderBy = "not_a_field".parse().unwrap();
        let err = order_by
            .validate_for_message(&site)
            .expect_err("a path that is not a Site field is rejected");
        match err {
            aip::ordering::Error::UnknownField(path) => assert_eq!(path, "not_a_field"),
            other => panic!("expected UnknownField for `not_a_field`, got {other:?}"),
        }
    }

    /// Builds a `prost_types::FieldMask` from the given paths.
    fn field_mask(paths: &[&str]) -> prost_types::FieldMask {
        prost_types::FieldMask {
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    /// Creates a shipper with the given display name and returns the stored
    /// resource (with its server-assigned name and timestamps).
    async fn create_shipper(server: &FreightServer, display_name: &str) -> Shipper {
        server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: display_name.to_owned(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("create_shipper succeeds")
            .into_inner()
    }

    /// A well-formed UUIDv4 `request_id`, the AIP-155 format the create requests
    /// advertise (`(google.api.field_info).format = UUID4`).
    const REQUEST_ID: &str = "49351204-7395-47f1-9681-d48044b48c71";

    #[tokio::test]
    async fn create_shipper_replays_on_same_request_id() {
        // AIP-155 (#94): a retry carrying the same `request_id` returns the
        // original shipper instead of minting a second one — a safe retry.
        let server = FreightServer::new();
        let request = CreateShipperRequest {
            shipper: Some(Shipper {
                display_name: "Acme".to_owned(),
                ..Default::default()
            }),
            request_id: REQUEST_ID.to_owned(),
            ..Default::default()
        };

        let first = server
            .create_shipper(Request::new(request.clone()))
            .await
            .expect("first create succeeds")
            .into_inner();
        let replay = server
            .create_shipper(Request::new(request))
            .await
            .expect("a replay with the same request_id succeeds")
            .into_inner();

        // Same resource (name and all): the second call created nothing new.
        assert_eq!(first, replay);
        assert_eq!(server.storage.list_shippers().len(), 1);
    }

    #[tokio::test]
    async fn create_shipper_rejects_conflicting_request_id() {
        use tonic_types::StatusExt as _;

        // AIP-155 (#94): the same `request_id` replayed with a *different* body is
        // a reuse conflict — rejected with ALREADY_EXISTS + AIP-193 details, and
        // the conflicting shipper is never created.
        let server = FreightServer::new();
        server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: "Acme".to_owned(),
                    ..Default::default()
                }),
                request_id: REQUEST_ID.to_owned(),
                ..Default::default()
            }))
            .await
            .expect("first create succeeds");

        let status = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: "Other".to_owned(),
                    ..Default::default()
                }),
                request_id: REQUEST_ID.to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("a conflicting body under the same request_id is rejected");

        assert_eq!(status.code(), tonic::Code::AlreadyExists);
        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "REQUEST_ID_CONFLICT");
        assert_eq!(info.domain, "aip-rs");
        assert_eq!(
            info.metadata.get("request_id").map(String::as_str),
            Some(REQUEST_ID)
        );
        // Only the first shipper exists.
        assert_eq!(server.storage.list_shippers().len(), 1);
    }

    #[tokio::test]
    async fn create_shipper_rejects_malformed_request_id() {
        use tonic_types::StatusExt as _;

        // AIP-155 / AIP-193: a `request_id` that is not a UUID is INVALID_ARGUMENT,
        // carrying the `REQUEST_ID_INVALID` reason; nothing is created.
        let server = FreightServer::new();
        let status = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: "Acme".to_owned(),
                    ..Default::default()
                }),
                request_id: "not-a-uuid".to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("a malformed request_id is rejected");

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "REQUEST_ID_INVALID");
        assert_eq!(info.domain, "aip-rs");
        assert!(server.storage.list_shippers().is_empty());
    }

    #[tokio::test]
    async fn create_site_replays_with_annotations_map() {
        // Regression: the replay check compares the request *structurally*, not
        // byte-for-byte. A `Site.annotations` map is a HashMap whose wire order is
        // non-deterministic across encodes, so a byte comparison could reject a
        // legitimate retry as a conflict. With several keys, a safe retry must
        // still replay (same site, no second create).
        let server = FreightServer::new();
        let request = CreateSiteRequest {
            parent: PARENT.to_owned(),
            site: Some(Site {
                display_name: "Depot".to_owned(),
                annotations: [
                    ("owner", "ops"),
                    ("region", "west"),
                    ("tier", "gold"),
                    ("zone", "a1"),
                ]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
                ..Default::default()
            }),
            request_id: REQUEST_ID.to_owned(),
            ..Default::default()
        };

        let first = server
            .create_site(Request::new(request.clone()))
            .await
            .expect("first create succeeds")
            .into_inner();
        let replay = server
            .create_site(Request::new(request))
            .await
            .expect("a replay carrying an annotations map is not a false conflict")
            .into_inner();

        assert_eq!(first, replay);
    }

    #[tokio::test]
    async fn create_shipper_without_request_id_is_not_idempotent() {
        // An absent `request_id` keeps the AIP-148 default: each call mints a new
        // system-assigned name, so two creates are two distinct shippers.
        let server = FreightServer::new();
        let a = create_shipper(&server, "Acme").await;
        let b = create_shipper(&server, "Acme").await;
        assert_ne!(a.name, b.name);
        assert_eq!(server.storage.list_shippers().len(), 2);
    }

    /// Applies an `UpdateShipper` with the given incoming shipper and mask,
    /// returning the updated resource.
    async fn update_shipper(server: &FreightServer, shipper: Shipper, mask: &[&str]) -> Shipper {
        server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(shipper),
                update_mask: Some(field_mask(mask)),
                ..Default::default()
            }))
            .await
            .expect("update_shipper succeeds")
            .into_inner()
    }

    #[tokio::test]
    async fn update_shipper_applies_update_mask_via_typed_facade() {
        // Exercises the typed `update` facade (#48) end-to-end through the handler:
        // a masked field changes and an unmasked field is left untouched.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        let name = created.name.clone();

        // (1) A masked field is changed; the OUTPUT_ONLY `create_time` survives the
        // typed-facade round-trip untouched.
        let changed = update_shipper(
            &server,
            Shipper {
                name: name.clone(),
                display_name: "Acme Corp".to_owned(),
                ..Default::default()
            },
            &["display_name"],
        )
        .await;
        assert_eq!(changed.display_name, "Acme Corp");
        assert_eq!(changed.create_time, created.create_time);

        // (2) An unmasked field is untouched: masking only `delete_time` leaves the
        // stored `display_name` in place though the request carries a different one.
        let untouched = update_shipper(
            &server,
            Shipper {
                name: name.clone(),
                display_name: "Ignored".to_owned(),
                ..Default::default()
            },
            &["delete_time"],
        )
        .await;
        assert_eq!(untouched.display_name, "Acme Corp");
    }

    #[tokio::test]
    async fn update_shipper_rejects_blanking_required_display_name() {
        use tonic_types::StatusExt as _;

        // AIP-203: an update whose mask names `display_name` but whose request
        // carries no value would blank a REQUIRED field. The `fieldbehavior`
        // primitive rejects it with INVALID_ARGUMENT + AIP-193 details, and the
        // stored resource is left untouched.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        let name = created.name.clone();

        let status = server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(Shipper {
                    name: name.clone(),
                    ..Default::default()
                }),
                update_mask: Some(field_mask(&["display_name"])),
                ..Default::default()
            }))
            .await
            .expect_err("blanking a required display_name is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        assert_eq!(bad.field_violations[0].field, "display_name");

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, "aip-rs");

        // The rejected update never reached storage: the stored display_name stands.
        let stored = server
            .get_shipper(Request::new(GetShipperRequest { name }))
            .await
            .expect("get_shipper succeeds")
            .into_inner();
        assert_eq!(stored.display_name, "Acme");
    }

    #[tokio::test]
    async fn update_shipper_runs_the_aip154_read_modify_write_cycle() {
        use tonic_types::StatusExt as _;

        // #93 / AIP-154: a Create stamps a content etag, an Update piggybacks it
        // back for the freshness check, and a stale token is rejected so a
        // concurrent writer can no longer silently clobber.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        assert!(!created.etag.is_empty(), "create stamps a content etag");

        // The read-modify-write with the etag the client just read succeeds, and
        // the server returns a *new* etag because the content changed.
        let updated = server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(Shipper {
                    name: created.name.clone(),
                    display_name: "Acme Corp".to_owned(),
                    etag: created.etag.clone(),
                    ..Default::default()
                }),
                update_mask: Some(field_mask(&["display_name"])),
                ..Default::default()
            }))
            .await
            .expect("update with a fresh etag succeeds")
            .into_inner();
        assert_eq!(updated.display_name, "Acme Corp");
        assert_ne!(
            updated.etag, created.etag,
            "a content change moves the etag",
        );

        // Replaying the original (now stale) etag is rejected with ABORTED — the
        // optimistic-concurrency guard that makes the cycle safe.
        let status = server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(Shipper {
                    name: created.name.clone(),
                    display_name: "Stale Write".to_owned(),
                    etag: created.etag.clone(),
                    ..Default::default()
                }),
                update_mask: Some(field_mask(&["display_name"])),
                ..Default::default()
            }))
            .await
            .expect_err("a stale etag is rejected");
        assert_eq!(status.code(), tonic::Code::Aborted);
        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "ETAG_MISMATCH");

        // The stale write never reached storage.
        let stored = server
            .get_shipper(Request::new(GetShipperRequest {
                name: created.name.clone(),
            }))
            .await
            .expect("get_shipper succeeds")
            .into_inner();
        assert_eq!(stored.display_name, "Acme Corp");
        assert_eq!(
            stored.etag, updated.etag,
            "the stored etag is the fresh one"
        );
    }

    #[tokio::test]
    async fn update_shipper_rejects_a_malformed_etag() {
        use tonic_types::StatusExt as _;

        // A token that could not have come from a prior read is INVALID_ARGUMENT,
        // not a concurrency conflict — distinct from the stale-etag ABORTED above.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        let status = server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(Shipper {
                    name: created.name,
                    display_name: "Acme Corp".to_owned(),
                    etag: "not-a-real-etag".to_owned(),
                    ..Default::default()
                }),
                update_mask: Some(field_mask(&["display_name"])),
                ..Default::default()
            }))
            .await
            .expect_err("a malformed etag is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "ETAG_MALFORMED");
    }

    #[tokio::test]
    async fn delete_shipper_honours_the_etag() {
        // Delete carries the etag on the request (it can't piggyback on the
        // resource). A stale token blocks the delete; the fresh one permits it.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        let updated = server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(Shipper {
                    name: created.name.clone(),
                    display_name: "Acme Corp".to_owned(),
                    etag: created.etag.clone(),
                    ..Default::default()
                }),
                update_mask: Some(field_mask(&["display_name"])),
                ..Default::default()
            }))
            .await
            .expect("update succeeds")
            .into_inner();

        // The original etag is now stale: deleting with it is ABORTED.
        let status = server
            .delete_shipper(Request::new(DeleteShipperRequest {
                name: created.name.clone(),
                etag: created.etag,
            }))
            .await
            .expect_err("a stale etag blocks the delete");
        assert_eq!(status.code(), tonic::Code::Aborted);

        // The fresh etag permits the delete, and the shipper is gone.
        server
            .delete_shipper(Request::new(DeleteShipperRequest {
                name: created.name.clone(),
                etag: updated.etag,
            }))
            .await
            .expect("a fresh etag permits the delete");
        let gone = server
            .get_shipper(Request::new(GetShipperRequest { name: created.name }))
            .await
            .expect_err("the deleted shipper is gone");
        assert_eq!(gone.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn create_shipper_missing_display_name_carries_aip193_details() {
        use tonic_types::StatusExt as _;

        // The fieldbehavior primitive validates REQUIRED fields and emits AIP-193
        // details: a `BadRequest` naming the field path and an `ErrorInfo` with
        // domain `"aip-rs"` (the primitive's own domain, not the service domain).
        let server = FreightServer::new();
        let status = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper::default()),
                ..Default::default()
            }))
            .await
            .expect_err("an empty display_name is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        assert_eq!(bad.field_violations.len(), 1);
        assert_eq!(bad.field_violations[0].field, "display_name");

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, "aip-rs");
    }

    #[tokio::test]
    async fn list_sites_unknown_order_by_field_carries_aip193_details() {
        use tonic_types::StatusExt as _;

        // An unknown ordering field flows through the `ordering` crate's AIP-193
        // `From<Error> for Status` (#16): the `BadRequest` names the field path
        // and the `ErrorInfo` carries the machine-readable reason + domain.
        let server = FreightServer::new();
        let status = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                order_by: "unknown_field".to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("an unknown order_by field is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        assert_eq!(bad.field_violations[0].field, "unknown_field");

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "ORDER_BY_UNKNOWN_FIELD");
        assert_eq!(info.domain, "aip-rs");
    }

    #[tokio::test]
    async fn list_sites_bad_parent_names_the_parent_field() {
        use tonic_types::StatusExt as _;

        // `validate_shipper_name` is shared by `name`- and `parent`-bearing
        // handlers; the BadRequest must point at the field the value came from.
        // `shippers/acme/sites/x` is a valid resource name but does not match the
        // `shippers/{shipper}` pattern, so it trips the server's policy check.
        let server = FreightServer::new();
        let status = server
            .list_sites(Request::new(ListSitesRequest {
                parent: "shippers/acme/sites/x".to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("a parent that is not a shipper name is rejected");
        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        assert_eq!(bad.field_violations[0].field, "parent");
    }

    #[tokio::test]
    async fn create_shipment_missing_endpoints_aggregates_field_violations() {
        use tonic_types::StatusExt as _;

        // A shipment with neither endpoint set trips both presence checks. The
        // `Validator` accumulates them, so the client gets *both* violations in a
        // single `BadRequest` — not just the first — carrying the service's own
        // domain (no aip-rs primitive covers these presence checks).
        let server = FreightServer::new();
        let status = server
            .create_shipment(Request::new(CreateShipmentRequest {
                parent: PARENT.to_owned(),
                shipment: Some(Shipment::default()),
                ..Default::default()
            }))
            .await
            .expect_err("a shipment missing both endpoints is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        let fields: Vec<&str> = bad
            .field_violations
            .iter()
            .map(|v| v.field.as_str())
            .collect();
        assert_eq!(
            fields,
            ["shipment.origin_site", "shipment.destination_site"]
        );

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, "freight.example.com");
    }

    #[tokio::test]
    async fn create_shipper_validate_only_previews_without_persisting() {
        // AIP-163: `validate_only` runs the full pipeline and returns the would-be
        // shipper — a system-assigned name and a stamped etag — but stores nothing,
        // so a later real create still mints a fresh shipper.
        let server = FreightServer::new();
        let preview = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: "Acme".to_owned(),
                    ..Default::default()
                }),
                validate_only: true,
                ..Default::default()
            }))
            .await
            .expect("a valid validate_only create returns the would-be shipper")
            .into_inner();
        // The preview is the would-be resource: the id is minted and the etag stamped.
        assert!(preview.name.starts_with("shippers/"));
        assert!(!preview.etag.is_empty());
        // Nothing was committed.
        assert!(server.storage.list_shippers().is_empty());

        // A subsequent real create succeeds and is the only stored shipper.
        let created = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper {
                    display_name: "Acme".to_owned(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("the real create succeeds")
            .into_inner();
        assert_eq!(server.storage.list_shippers().len(), 1);
        // The preview did not reserve the id — the committed shipper has its own.
        assert_ne!(preview.name, created.name);
    }

    #[tokio::test]
    async fn validate_only_create_failure_is_byte_identical() {
        // AIP-163: a request that would fail fails *identically* with or without
        // the flag — same code, message, and AIP-193 detail bytes — because
        // validation runs unconditionally before the commit gate.
        let server = FreightServer::new();
        let invalid = |validate_only| CreateShipmentRequest {
            parent: PARENT.to_owned(),
            shipment: Some(Shipment::default()),
            validate_only,
            ..Default::default()
        };

        let real = server
            .create_shipment(Request::new(invalid(false)))
            .await
            .expect_err("the real create is rejected");
        let preview = server
            .create_shipment(Request::new(invalid(true)))
            .await
            .expect_err("the validate_only create is rejected identically");

        assert_eq!(real.code(), preview.code());
        assert_eq!(real.message(), preview.message());
        // The AIP-193 detail payload (the BadRequest + ErrorInfo) is byte-for-byte
        // the same — the flag does not branch the validation path.
        assert_eq!(real.details(), preview.details());
    }

    /// Lists sites under `PARENT` carrying an AIP-160 `filter`, returning their
    /// display names in the order the server produced them.
    async fn list_filtered_display_names(server: &FreightServer, filter: &str) -> Vec<String> {
        let resp = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                filter: filter.to_owned(),
                order_by: "display_name".to_owned(),
                ..Default::default()
            }))
            .await
            .expect("list_sites succeeds")
            .into_inner();
        resp.sites.into_iter().map(|s| s.display_name).collect()
    }

    #[tokio::test]
    async fn filter_returns_only_matching_site_from_sqlite() {
        // The headline tracer-bullet path (#39): `display_name = "Alpha"` is
        // type-checked, transpiled to a parameterized Predicate, and run inside
        // SQLite, which returns just the matching row.
        let server = FreightServer::new();
        for name in ["Alpha", "Bravo", "Charlie"] {
            seed_site(&server, name, 0.0).await;
        }
        assert_eq!(
            list_filtered_display_names(&server, r#"display_name = "Alpha""#).await,
            ["Alpha"],
        );
    }

    #[tokio::test]
    async fn filter_conjunction_binds_both_literals() {
        // `AND` over two `=` leaves binds two parameters; contradictory equalities
        // match nothing, proving both binds reach SQLite.
        let server = FreightServer::new();
        for name in ["Alpha", "Bravo"] {
            seed_site(&server, name, 0.0).await;
        }
        assert!(list_filtered_display_names(
            &server,
            r#"display_name = "Alpha" AND display_name = "Bravo""#
        )
        .await
        .is_empty(),);
    }

    #[tokio::test]
    async fn filter_disjunction_matches_either_branch() {
        // `OR` now lowers to SQL (#40): a disjunction returns the union of its
        // branches.
        let server = FreightServer::new();
        for name in ["Alpha", "Bravo", "Charlie"] {
            seed_site(&server, name, 0.0).await;
        }
        assert_eq!(
            list_filtered_display_names(
                &server,
                r#"display_name = "Alpha" OR display_name = "Charlie""#,
            )
            .await,
            ["Alpha", "Charlie"],
        );
    }

    #[tokio::test]
    async fn filter_by_numeric_latitude() {
        // A numeric comparison over the nested `lat_lng.latitude` path: `> 0`
        // keeps only the northern sites. The `Double > Int` overload lets the
        // bare `0` literal compare against the double column.
        let server = FreightServer::new();
        seed_site(&server, "north", 60.0).await;
        seed_site(&server, "south", -30.0).await;
        seed_site(&server, "equator", 0.0).await;
        assert_eq!(
            list_filtered_display_names(&server, "lat_lng.latitude > 0").await,
            ["north"],
        );
    }

    #[tokio::test]
    async fn filter_by_timestamp_create_time() {
        // `create_time` is server-set to `now()` and stored as RFC3339 text, so a
        // far-past bound matches every site and a far-future bound matches none —
        // proving the bound timestamp literal runs inside SQLite.
        let server = FreightServer::new();
        for name in ["Alpha", "Bravo"] {
            seed_site(&server, name, 0.0).await;
        }
        assert_eq!(
            list_filtered_display_names(&server, r#"create_time > "2000-01-01T00:00:00Z""#).await,
            ["Alpha", "Bravo"],
        );
        assert!(
            list_filtered_display_names(&server, r#"create_time > "2999-01-01T00:00:00Z""#)
                .await
                .is_empty(),
        );
    }

    #[tokio::test]
    async fn filter_by_enum_state() {
        // A reflective enum filter: `state = STATE_ACTIVE` binds the value name
        // and returns only the active sites.
        let server = FreightServer::new();
        seed_site_with_state(&server, "Alpha", site::State::Active).await;
        seed_site_with_state(&server, "Bravo", site::State::Inactive).await;
        seed_site_with_state(&server, "Charlie", site::State::Active).await;
        assert_eq!(
            list_filtered_display_names(&server, "state = STATE_ACTIVE").await,
            ["Alpha", "Charlie"],
        );
    }

    #[tokio::test]
    async fn filter_by_map_annotation_key_presence() {
        // `:` on the `annotations` map tests key presence via SQLite's
        // `json_each` (#41): `annotations:owner` keeps only the sites carrying
        // that key, whatever its value.
        let server = FreightServer::new();
        seed_site_with_metadata(&server, "Alpha", &[("owner", "ops")], &[]).await;
        seed_site_with_metadata(&server, "Bravo", &[("region", "west")], &[]).await;
        seed_site_with_metadata(&server, "Charlie", &[("owner", "sales")], &[]).await;
        assert_eq!(
            list_filtered_display_names(&server, "annotations:owner").await,
            ["Alpha", "Charlie"],
        );
    }

    #[tokio::test]
    async fn filter_by_list_tag_membership() {
        // `:` on the `tags` list tests element presence via `json_each` (#41):
        // `tags:refrigerated` keeps only the sites carrying that tag.
        let server = FreightServer::new();
        seed_site_with_metadata(&server, "Alpha", &[], &["refrigerated", "hazmat"]).await;
        seed_site_with_metadata(&server, "Bravo", &[], &["bulk"]).await;
        seed_site_with_metadata(&server, "Charlie", &[], &["refrigerated"]).await;
        assert_eq!(
            list_filtered_display_names(&server, "tags:refrigerated").await,
            ["Alpha", "Charlie"],
        );
    }

    #[tokio::test]
    async fn filter_by_string_substring() {
        // `:` on a string column is a substring match (#41): `display_name:lph`
        // keeps only the sites whose display name contains "lph".
        let server = FreightServer::new();
        for name in ["Alpha", "Bravo", "Charlie"] {
            seed_site(&server, name, 0.0).await;
        }
        assert_eq!(
            list_filtered_display_names(&server, "display_name:lph").await,
            ["Alpha"],
        );
    }

    /// The README's advertised `ListSites` filter corpus — one filter per
    /// operator the matcher and the SQL path both implement, including the
    /// three-valued (`NULL`) cases (`annotations.owner` on a site lacking the
    /// key, `lat_lng.latitude` on a site lacking a location) where the two paths
    /// must agree that an absent operand excludes the row.
    const FILTER_CORPUS: &[&str] = &[
        r#"display_name = "Alpha""#,
        r#"display_name = "Alpha" OR display_name = "Bravo""#,
        r#"display_name = "Alpha" AND display_name = "Bravo""#,
        "lat_lng.latitude > 0",
        r#"create_time > "2000-01-01T00:00:00Z""#,
        r#"create_time > "2999-01-01T00:00:00Z""#,
        "state = STATE_ACTIVE",
        "state != STATE_ACTIVE",
        "NOT state = STATE_ACTIVE",
        "annotations:owner",
        r#"annotations.owner = "ops""#,
        "tags:refrigerated",
        "display_name:lph",
        r#"display_name = "Alpha" OR tags:refrigerated"#,
    ];

    /// `names` sorted, so two list-method results compare independently of order.
    fn sorted(mut names: Vec<String>) -> Vec<String> {
        names.sort();
        names
    }

    /// Seed a heterogeneous Site corpus under `PARENT`, returning the created
    /// Sites (each with its server-set `name` / `create_time`). The shapes vary
    /// across state, location, annotations, and tags so every corpus filter both
    /// keeps and drops at least one site — and some sites lack a key / a location,
    /// to exercise the `NULL`/absent agreement.
    async fn seed_filter_corpus(server: &FreightServer) -> Vec<Site> {
        let annotations = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let specs = vec![
            Site {
                display_name: "Alpha".to_owned(),
                state: site::State::Active as i32,
                lat_lng: Some(LatLng {
                    latitude: 60.0,
                    longitude: 0.0,
                }),
                annotations: annotations(&[("owner", "ops")]),
                tags: vec!["refrigerated".to_owned(), "hazmat".to_owned()],
                ..Default::default()
            },
            Site {
                display_name: "Bravo".to_owned(),
                state: site::State::Inactive as i32,
                lat_lng: Some(LatLng {
                    latitude: -30.0,
                    longitude: 0.0,
                }),
                annotations: annotations(&[("region", "west")]),
                tags: vec!["bulk".to_owned()],
                ..Default::default()
            },
            Site {
                display_name: "Charlie".to_owned(),
                state: site::State::Active as i32,
                lat_lng: Some(LatLng {
                    latitude: 0.0,
                    longitude: 0.0,
                }),
                tags: vec!["refrigerated".to_owned()],
                ..Default::default()
            },
            // No location and no `owner` annotation, but a display name that still
            // contains the `lph` substring (`De-lph-i`).
            Site {
                display_name: "Delphi".to_owned(),
                state: site::State::Unspecified as i32,
                annotations: annotations(&[("region", "east")]),
                ..Default::default()
            },
        ];

        let mut created = Vec::new();
        for site in specs {
            let response = server
                .create_site(Request::new(CreateSiteRequest {
                    parent: PARENT.to_owned(),
                    site: Some(site),
                    ..Default::default()
                }))
                .await
                .expect("create_site succeeds")
                .into_inner();
            created.push(response);
        }
        created
    }

    #[tokio::test]
    async fn in_memory_matcher_agrees_with_sqlite_over_the_filter_corpus() {
        // Issue #92: the in-memory reflective matcher (`aip::filtering::matches`)
        // and the `aip-sql` + SQLite path must select the *same* Sites for every
        // advertised filter — so an AIP-160 Filter means one thing whether a caller
        // has a database or not. For each filter we compare the display names
        // SQLite returns (`ListSites`, whose parent scope and soft-delete drop
        // nothing here) against the ones the matcher keeps over the same corpus.
        let server = FreightServer::new();
        let corpus = seed_filter_corpus(&server).await;
        let declarations = site_declarations();

        for filter in FILTER_CORPUS {
            let from_sqlite = sorted(list_filtered_display_names(&server, filter).await);

            let checked = aip::filtering::check(filter, &declarations)
                .unwrap_or_else(|error| panic!("corpus filter {filter:?} type-checks: {error}"));
            let from_matcher = sorted(
                corpus
                    .iter()
                    .filter(|site| {
                        aip::filtering::matches(&checked, &declarations, *site)
                            .unwrap_or_else(|error| panic!("matcher evaluates {filter:?}: {error}"))
                    })
                    .map(|site| site.display_name.clone())
                    .collect(),
            );

            assert_eq!(
                from_matcher, from_sqlite,
                "matcher and SQLite disagree on filter {filter:?}",
            );
        }
    }

    #[tokio::test]
    async fn list_sites_scopes_to_parent_in_sql() {
        // The parent scope now runs in the SQL `WHERE` (#43, `scope_to_parent`),
        // not an in-memory post-filter: sites under a different shipper — including
        // one whose name is a string prefix of the parent (`shippers/acme2`) — are
        // excluded, proving the bound `LIKE 'shippers/acme/%'` respects the segment
        // boundary.
        let server = FreightServer::new();
        seed_site(&server, "Mine", 0.0).await; // under PARENT (`shippers/acme`)
        for other_parent in ["shippers/other", "shippers/acme2"] {
            server
                .create_site(Request::new(CreateSiteRequest {
                    parent: other_parent.to_owned(),
                    site: Some(Site {
                        display_name: "Theirs".to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }))
                .await
                .expect("create_site succeeds");
        }
        assert_eq!(list_display_names(&server, "display_name").await, ["Mine"]);
    }

    #[tokio::test]
    async fn list_sites_excludes_soft_deleted_in_sql() {
        // The soft-delete predicate `delete_time IS NULL` runs in SQL (#43): a site
        // carrying a `delete_time` is dropped from the listing. `DeleteSite` is not
        // yet wired, so the soft-deleted row is seeded straight into the store.
        let server = FreightServer::new();
        seed_site(&server, "Live", 0.0).await;
        server.storage.put_site(Site {
            name: format!("{PARENT}/sites/deleted-1"),
            display_name: "Gone".to_owned(),
            delete_time: Some(now()),
            ..Default::default()
        });
        assert_eq!(list_display_names(&server, "display_name").await, ["Live"]);
    }

    /// Creates a shipment under `PARENT` with the given origin/destination site
    /// references and annotations, returning the stored resource.
    async fn create_shipment(
        server: &FreightServer,
        origin: &str,
        destination: &str,
        annotations: &[(&str, &str)],
    ) -> Shipment {
        server
            .create_shipment(Request::new(CreateShipmentRequest {
                parent: PARENT.to_owned(),
                shipment: Some(Shipment {
                    origin_site: origin.to_owned(),
                    destination_site: destination.to_owned(),
                    annotations: annotations
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("create_shipment succeeds")
            .into_inner()
    }

    /// Lists shipments under `PARENT` carrying an AIP-160 `filter`, returning their
    /// `origin_site` references.
    async fn list_filtered_origins(server: &FreightServer, filter: &str) -> Vec<String> {
        let resp = server
            .list_shipments(Request::new(ListShipmentsRequest {
                parent: PARENT.to_owned(),
                filter: filter.to_owned(),
                ..Default::default()
            }))
            .await
            .expect("list_shipments succeeds")
            .into_inner();
        resp.shipments.into_iter().map(|s| s.origin_site).collect()
    }

    #[tokio::test]
    async fn list_shipments_scopes_to_parent_with_no_filter() {
        // `ListShipments` is SQLite-backed and composes scope + soft-delete (#43).
        // With no filter it lists every in-scope shipment; one created under a
        // different shipper is excluded by the parent scope.
        let site = "shippers/acme/sites/x";
        let server = FreightServer::new();
        create_shipment(&server, site, site, &[]).await;
        create_shipment(&server, site, site, &[]).await;
        server
            .create_shipment(Request::new(CreateShipmentRequest {
                parent: "shippers/other".to_owned(),
                shipment: Some(Shipment {
                    origin_site: site.to_owned(),
                    destination_site: site.to_owned(),
                    ..Default::default()
                }),
                ..Default::default()
            }))
            .await
            .expect("create_shipment succeeds");
        assert_eq!(list_filtered_origins(&server, "").await.len(), 2);
    }

    #[tokio::test]
    async fn list_shipments_filters_in_sqlite() {
        // The user filter composes with the server predicates and runs in SQLite:
        // `origin_site = X` returns only the matching shipment.
        let a = "shippers/acme/sites/a";
        let b = "shippers/acme/sites/b";
        let server = FreightServer::new();
        create_shipment(&server, a, b, &[]).await;
        create_shipment(&server, b, a, &[]).await;
        assert_eq!(
            list_filtered_origins(&server, &format!(r#"origin_site = "{a}""#)).await,
            [a],
        );
    }

    #[tokio::test]
    async fn list_shipments_has_operator_over_annotations() {
        // The has operator on the `annotations` map (`json_each`) composes through
        // the same path: only the shipment carrying the key is returned.
        let site = "shippers/acme/sites/x";
        let server = FreightServer::new();
        create_shipment(&server, site, site, &[("priority", "high")]).await;
        create_shipment(&server, site, site, &[("region", "west")]).await;
        assert_eq!(
            list_filtered_origins(&server, "annotations:priority").await,
            [site]
        );
    }

    #[tokio::test]
    async fn list_shipments_rejects_invalid_filter() {
        // An unfilterable identifier is rejected with `INVALID_ARGUMENT`, the same
        // gate `ListSites` applies — the filter never reaches SQL.
        let server = FreightServer::new();
        let status = server
            .list_shipments(Request::new(ListShipmentsRequest {
                parent: PARENT.to_owned(),
                filter: r#"not_a_field = "x""#.to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("an unknown filter field is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    // ----- GetShipper authorization (AIP-211, #67) -----

    use aip::iam::proto::{Binding, Policy};

    /// Lock `resource` down to exactly `members` (granting an arbitrary role)
    /// through the shared policy store, so the authorization gate admits only those
    /// callers. A resource with no policy stays public.
    fn lock_resource(server: &FreightServer, resource: &str, members: &[&str]) {
        let policy = Policy {
            version: 1,
            bindings: vec![Binding {
                role: "roles/viewer".to_owned(),
                members: members.iter().map(|m| (*m).to_owned()).collect(),
                condition: None,
            }],
            etag: Vec::new(),
            audit_configs: Vec::new(),
        };
        server
            .policies
            .set_checked(resource.to_owned(), policy)
            .expect("seed policy");
    }

    /// Drive `GetShipper` for `name` as `caller` (an `x-freight-caller` Member
    /// string, or `None` for an anonymous caller).
    async fn get_as(
        server: &FreightServer,
        name: &str,
        caller: Option<&str>,
    ) -> Result<Shipper, Status> {
        let mut request = Request::new(GetShipperRequest {
            name: name.to_owned(),
        });
        if let Some(caller) = caller {
            request
                .metadata_mut()
                .insert(CALLER_METADATA_KEY, caller.parse().expect("metadata value"));
        }
        server.get_shipper(request).await.map(Response::into_inner)
    }

    #[tokio::test]
    async fn get_shipper_is_public_until_a_policy_is_attached() {
        // A shipper with no Policy attached is public in the demo (mirroring the
        // open ListShippers), so an anonymous caller reads it.
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        let got = get_as(&server, &created.name, None)
            .await
            .expect("an unprotected shipper is readable");
        assert_eq!(got, created);
    }

    #[tokio::test]
    async fn get_shipper_denies_an_unauthorized_caller_on_a_locked_shipper() {
        use tonic_types::StatusExt as _;

        // Lock the shipper down to alice. She reads it; bob (and an anonymous
        // caller) get the canonical non-leaking PERMISSION_DENIED (#67).
        let server = FreightServer::new();
        let created = create_shipper(&server, "Acme").await;
        lock_resource(&server, &created.name, &["user:alice@example.com"]);

        let allowed = get_as(&server, &created.name, Some("user:alice@example.com"))
            .await
            .expect("the granted member reads the shipper");
        assert_eq!(allowed, created);

        for caller in [Some("user:bob@example.com"), None] {
            let status = get_as(&server, &created.name, caller)
                .await
                .expect_err("an ungranted caller is denied");
            assert_eq!(status.code(), tonic::Code::PermissionDenied);
            // The non-leaking message hides whether the resource exists.
            assert_eq!(
                status.message(),
                format!(
                    "Permission 'freight.shippers.get' denied on resource '{}' \
                     (or it might not exist).",
                    created.name
                ),
            );
            let info = status
                .get_details_error_info()
                .expect("an ErrorInfo is attached (AIP-193 MUST)");
            assert_eq!(info.reason, "IAM_PERMISSION_DENIED");
            assert_eq!(info.domain, "aip-rs");
        }
    }

    #[tokio::test]
    async fn get_shipper_denial_does_not_leak_existence() {
        use tonic_types::StatusExt as _;

        // Non-leaking: an unauthorized caller who also cannot read the parent
        // collection's children gets the *same* PERMISSION_DENIED whether the
        // shipper exists or not — so a missing resource is indistinguishable from a
        // forbidden one. `shippers/ghost` was never created; both it and the parent
        // collection are locked against bob.
        let server = FreightServer::new();
        let existing = create_shipper(&server, "Acme").await;
        lock_resource(&server, &existing.name, &["user:alice@example.com"]);
        lock_resource(&server, "shippers/ghost", &["user:alice@example.com"]);
        lock_resource(&server, SHIPPERS_COLLECTION, &["user:alice@example.com"]);

        let on_existing = get_as(&server, &existing.name, Some("user:bob@example.com"))
            .await
            .expect_err("denied on the existing shipper");
        let on_missing = get_as(&server, "shippers/ghost", Some("user:bob@example.com"))
            .await
            .expect_err("denied on the missing shipper");

        // Same code and same machine-readable reason — no NOT_FOUND tell.
        assert_eq!(on_existing.code(), tonic::Code::PermissionDenied);
        assert_eq!(on_missing.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            on_missing
                .get_details_error_info()
                .expect("ErrorInfo")
                .reason,
            "IAM_PERMISSION_DENIED",
        );
    }

    #[tokio::test]
    async fn get_shipper_reveals_not_found_when_caller_may_read_the_parent() {
        use tonic_types::StatusExt as _;

        // AIP-211 fallback: a caller unauthorized on the (missing) resource but
        // authorized to read the parent collection's children is allowed to learn
        // it does not exist — NOT_FOUND, not PERMISSION_DENIED. `shippers/ghost` is
        // locked to alice; the parent collection grants bob.
        let server = FreightServer::new();
        lock_resource(&server, "shippers/ghost", &["user:alice@example.com"]);
        lock_resource(&server, SHIPPERS_COLLECTION, &["user:bob@example.com"]);

        let status = get_as(&server, "shippers/ghost", Some("user:bob@example.com"))
            .await
            .expect_err("the missing shipper is reported");
        assert_eq!(status.code(), tonic::Code::NotFound);
        assert_eq!(
            status.get_details_error_info().expect("ErrorInfo").reason,
            "IAM_RESOURCE_NOT_FOUND",
        );
    }
}
