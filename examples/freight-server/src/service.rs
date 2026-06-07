//! The `FreightService` gRPC implementation — the demo's whole point.
//!
//! The Shipper standard methods (Get/List/Create/Update/Delete) are wired up as
//! the worked reference; every place an aip-rs primitive belongs is marked with
//! a `TODO(aip #N)` tied to its tracking issue, so the handlers tighten up as the
//! per-feature crates land. Site, Shipment, and the batch method return
//! `Unimplemented` until they follow the same pattern.

use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use aip::ordering::{OrderBy, OrderByRequest};
use aip::pagination::{PageRequest, PageToken};
use prost_types::Timestamp;
use tonic::{Request, Response, Status};

use crate::proto::einride::example::freight::v1::{
    freight_service_server::FreightService, BatchGetSitesRequest, BatchGetSitesResponse,
    CreateShipmentRequest, CreateShipperRequest, CreateSiteRequest, DeleteShipmentRequest,
    DeleteShipperRequest, DeleteSiteRequest, GetShipmentRequest, GetShipperRequest, GetSiteRequest,
    ListShipmentsRequest, ListShipmentsResponse, ListShippersRequest, ListShippersResponse,
    ListSitesRequest, ListSitesResponse, Shipment, Shipper, Site, UpdateShipmentRequest,
    UpdateShipperRequest, UpdateSiteRequest,
};
use crate::reflect;
use crate::storage::Storage;

/// The shipper collection ID — the root segment of every shipper resource name.
const SHIPPERS_COLLECTION: &str = "shippers";

/// The fully-qualified `Shipper` message type, used to look up its reflective
/// descriptor for field-mask validation and application.
const SHIPPER_TYPE: &str = "einride.example.freight.v1.Shipper";

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
#[derive(Default)]
pub struct FreightServer {
    storage: Storage,
}

impl FreightServer {
    /// A server backed by an empty store.
    pub fn new() -> Self {
        Self {
            storage: Storage::new(),
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
        let name = request.into_inner().name;
        validate_shipper_name(&name)?;
        self.storage
            .get_shipper(&name)
            .map(Response::new)
            .ok_or_else(|| Status::not_found(format!("shipper `{name}` not found")))
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
        let mut shipper = request
            .into_inner()
            .shipper
            .ok_or_else(|| Status::invalid_argument("shipper is required"))?;
        if shipper.display_name.is_empty() {
            // TODO(aip #16): surface this as a structured AIP-193 BadRequest
            // field violation rather than a plain message.
            return Err(Status::invalid_argument("shipper.display_name is required"));
        }
        // Mint a system-assigned resource ID (a UUIDv4) per AIP-148.
        // `CreateShipperRequest` has no `shipper_id` field, so there is no
        // user-supplied id to validate here; `validate_user_settable` guards
        // that path wherever a request later exposes one.
        let id = aip::resourceid::generate_system();
        // Format the canonical resource name `shippers/{shipper}` from its
        // pattern (AIP-122) rather than hand-concatenating the segments.
        let shipper_pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}");
        shipper.name = aip::resourcename::Pattern::parse(&shipper_pattern)
            .expect("the shipper collection pattern is valid")
            .format([("shipper", id.as_str())])
            .expect("a generated shipper id formats into the pattern");
        let ts = now();
        shipper.create_time = Some(ts);
        shipper.update_time = Some(ts);
        shipper.delete_time = None;
        self.storage.put_shipper(shipper.clone());
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

        // Apply the AIP-134 update mask via the field-mask primitive. The mask is
        // validated against the `Shipper` descriptor, then the request's shipper
        // is merged into the stored one: an empty mask copies the populated
        // fields, `*` is a full replacement, and a named path absent from the
        // request clears that field. The crate is reflective, so we transcode the
        // generated `Shipper`s to `DynamicMessage` and back.
        let descriptor = reflect::descriptor(SHIPPER_TYPE);
        let mask = req.update_mask.unwrap_or_default();
        aip::fieldmask::validate(&mask, &descriptor)
            .map_err(|e| Status::invalid_argument(format!("invalid update_mask: {e}")))?;
        let mut merged = reflect::to_dynamic(&descriptor, &existing);
        let incoming = reflect::to_dynamic(&descriptor, &incoming);
        aip::fieldmask::update(&mask, &mut merged, &incoming)
            .map_err(|e| Status::invalid_argument(format!("update_mask: {e}")))?;
        let mut shipper: Shipper = reflect::from_dynamic(&merged);

        // The OUTPUT_ONLY timestamps are server-owned: a client mask must not move
        // `create_time`/`delete_time`, and every update stamps `update_time`.
        shipper.create_time = existing.create_time;
        shipper.update_time = Some(now());
        shipper.delete_time = existing.delete_time;
        self.storage.put_shipper(shipper.clone());
        Ok(Response::new(shipper))
    }

    async fn delete_shipper(
        &self,
        request: Request<DeleteShipperRequest>,
    ) -> Result<Response<Shipper>, Status> {
        let name = request.into_inner().name;
        // Soft delete (AIP-164) is deferred; this is a hard delete.
        validate_shipper_name(&name)?;
        self.storage
            .remove_shipper(&name)
            .map(Response::new)
            .ok_or_else(|| Status::not_found(format!("shipper `{name}` not found")))
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
        validate_shipper_name(&req.parent)?;

        // Parse and validate the AIP-132 `order_by` against the allow-list of
        // sortable Site paths (#9's `validate_for_paths`, not the descriptor-based
        // `validate_for_message` of #10). Bad syntax or an unknown ordering field
        // is an InvalidArgument.
        let order_by = aip::ordering::parse_order_by(&req)
            .map_err(|e| Status::invalid_argument(format!("invalid order_by: {e}")))?;
        order_by
            .validate_for_paths(SORTABLE_SITE_PATHS)
            .map_err(|e| Status::invalid_argument(format!("invalid order_by: {e}")))?;

        // Offset pagination (AIP-158). `order_by` is a non-pagination field, so
        // the request checksum `parse_page` computes covers it: changing it
        // mid-pagination flips the checksum and the now-stale token is rejected.
        let page = parse_page(&req)?;

        // Sites under this parent, in the store's stable resource-name order, then
        // sorted by `order_by`. The sort is stable, so the name order breaks ties
        // and the ordering stays consistent across pages.
        // TODO(aip #11): apply an AIP-160 filter here once `ListSitesRequest`
        // gains a `filter` field — the `aip::filtering` pipeline is ready.
        let mut sites: Vec<Site> = self
            .storage
            .list_sites()
            .into_iter()
            .filter(|site| aip::resourcename::has_parent(&site.name, &req.parent))
            .collect();
        sort_sites(&mut sites, &order_by);

        let total = sites.len();
        let start = usize::try_from(page.token.offset).unwrap_or(0).min(total);
        let end = start.saturating_add(page.size).min(total);
        // Only hand back a `next_page_token` when results remain past this page.
        let next_page_token = if end < total {
            page.token.next(page.size as i32).encode()
        } else {
            String::new()
        };
        let sites = sites.drain(start..end).collect();
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
        validate_shipper_name(&req.parent)?;
        let mut site = req
            .site
            .ok_or_else(|| Status::invalid_argument("site is required"))?;
        if site.display_name.is_empty() {
            return Err(Status::invalid_argument("site.display_name is required"));
        }

        // The validated `parent` binds the `{shipper}` of the canonical site
        // pattern; mint a system-assigned `{site}` id (a UUIDv4, per AIP-148) and
        // format the full resource name from the pattern (AIP-122) rather than
        // hand-concatenating the segments.
        let shipper_pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}");
        let shipper_id = aip::resourcename::Pattern::parse(&shipper_pattern)
            .expect("the shipper collection pattern is valid")
            .match_name(&req.parent)
            .and_then(|caps| caps.get("shipper"))
            .expect("parent validated to match the shipper pattern")
            .to_owned();
        let id = aip::resourceid::generate_system();
        let site_pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}/sites/{{site}}");
        site.name = aip::resourcename::Pattern::parse(&site_pattern)
            .expect("the site collection pattern is valid")
            .format([("shipper", shipper_id.as_str()), ("site", id.as_str())])
            .expect("a generated site id formats into the pattern");

        let ts = now();
        site.create_time = Some(ts);
        site.update_time = Some(ts);
        site.delete_time = None;
        self.storage.put_site(site.clone());
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
        _: Request<ListShipmentsRequest>,
    ) -> Result<Response<ListShipmentsResponse>, Status> {
        // TODO(aip #11): apply the AIP-160 filter here once ListShipments is
        // wired — the same `aip::filtering` seam as ListSites, blocked on the
        // method and on `ListShipmentsRequest` gaining a `filter` field.
        Err(unimplemented("ListShipments"))
    }

    async fn create_shipment(
        &self,
        _: Request<CreateShipmentRequest>,
    ) -> Result<Response<Shipment>, Status> {
        Err(unimplemented("CreateShipment"))
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

/// Lets `aip::ordering::parse_order_by` read the AIP-132 `order_by` field off
/// the generated request.
impl OrderByRequest for ListSitesRequest {
    fn order_by(&self) -> &str {
        &self.order_by
    }
}

/// Sorts `sites` in place by an AIP-132 `order_by`, breaking ties by resource
/// name so the order is total and stable across pages — independent of the
/// store's iteration order. An empty `order_by` leaves the store's
/// resource-name order untouched.
fn sort_sites(sites: &mut [Site], order_by: &OrderBy) {
    if order_by.fields.is_empty() {
        return;
    }
    sites.sort_by(|a, b| {
        for field in &order_by.fields {
            let ordering = compare_site_field(a, b, &field.path);
            let ordering = if field.desc {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        // Resource names are unique, so this makes the ordering total: equal
        // `order_by` keys fall back to a fixed name order on every page.
        a.name.cmp(&b.name)
    });
}

/// Compares two sites by a single Site field path.
///
/// Every path reaching here is one of [`SORTABLE_SITE_PATHS`] — the `order_by`
/// was validated against that allow-list — so an unrecognised path is
/// unreachable and compares Equal.
fn compare_site_field(a: &Site, b: &Site, path: &str) -> Ordering {
    match path {
        "name" => a.name.cmp(&b.name),
        "display_name" => a.display_name.cmp(&b.display_name),
        "create_time" => cmp_timestamp(&a.create_time, &b.create_time),
        "update_time" => cmp_timestamp(&a.update_time, &b.update_time),
        "lat_lng.latitude" => latitude(a).total_cmp(&latitude(b)),
        "lat_lng.longitude" => longitude(a).total_cmp(&longitude(b)),
        _ => Ordering::Equal,
    }
}

/// Orders two optional timestamps by `(seconds, nanos)`, with an unset time
/// sorting before any set time.
fn cmp_timestamp(a: &Option<Timestamp>, b: &Option<Timestamp>) -> Ordering {
    let key = |t: &Option<Timestamp>| t.as_ref().map(|t| (t.seconds, t.nanos));
    key(a).cmp(&key(b))
}

/// The site's latitude, or `0.0` when it carries no location.
fn latitude(site: &Site) -> f64 {
    site.lat_lng.as_ref().map_or(0.0, |ll| ll.latitude)
}

/// The site's longitude, or `0.0` when it carries no location.
fn longitude(site: &Site) -> f64 {
    site.lat_lng.as_ref().map_or(0.0, |ll| ll.longitude)
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
fn parse_page<M: PageRequest + prost::Name>(request: &M) -> Result<Page, Status> {
    let checksum = request_checksum_of(request)?;
    let token = PageToken::parse(request, checksum)
        .map_err(|e| Status::invalid_argument(format!("invalid page_token: {e}")))?;
    let size = effective_page_size(request.page_size())?;
    Ok(Page { token, size })
}

/// Computes [`aip::pagination::request_checksum`] for a concrete request.
///
/// The library's reflective surface is `DynamicMessage`-based (ADR-0001), but the
/// generated request types carry no reflection. We transcode the request to a
/// `DynamicMessage` via the [`reflect`] bridge, then checksum it. The request's
/// fully-qualified message name is derived from its type via [`prost::Name`], so
/// it can't drift from the actual message.
fn request_checksum_of<M: prost::Name>(request: &M) -> Result<u32, Status> {
    let dynamic = reflect::to_dynamic(&reflect::descriptor(&M::full_name()), request);
    aip::pagination::request_checksum(&dynamic)
        .map_err(|e| Status::internal(format!("compute request checksum: {e}")))
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

/// Validates that `name` is a well-formed shipper resource name (AIP-122): a
/// valid resource name that matches the `shippers/{shipper}` pattern. Returns
/// `INVALID_ARGUMENT` otherwise.
fn validate_shipper_name(name: &str) -> Result<(), Status> {
    aip::resourcename::validate(name)
        .map_err(|e| Status::invalid_argument(format!("invalid resource name `{name}`: {e}")))?;
    let pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}");
    if !aip::resourcename::is_match(&pattern, name) {
        return Err(Status::invalid_argument(format!(
            "name `{name}` must match the pattern `{pattern}`"
        )));
    }
    Ok(())
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
    use crate::proto::google::r#type::LatLng;

    /// A shipper parent name; the demo does not require the shipper to exist in
    /// storage for `CreateSite`/`ListSites`, only that the name is well-formed.
    const PARENT: &str = "shippers/acme";

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

    /// The fully-qualified `Site` message type, for its reflective descriptor.
    const SITE_TYPE: &str = "einride.example.freight.v1.Site";

    #[test]
    fn sortable_site_paths_resolve_on_the_site_descriptor() {
        // `ListSites` gates `order_by` with the curated `validate_for_paths`
        // allow-list (#9), since the in-memory `sort_sites` only knows those
        // paths. `validate_for_message` (#10) guards the allow-list itself: every
        // sortable path must be a real `Site` field, so the allow-list can't
        // silently drift from the proto.
        let site = reflect::descriptor(SITE_TYPE);
        let order_by: OrderBy = SORTABLE_SITE_PATHS
            .join(",")
            .parse()
            .expect("the allow-list is valid order_by syntax");
        order_by
            .validate_for_message(&site)
            .expect("every sortable Site path resolves on the Site descriptor");
    }

    #[test]
    fn validate_for_message_rejects_unknown_site_path() {
        let site = reflect::descriptor(SITE_TYPE);
        let order_by: OrderBy = "not_a_field".parse().unwrap();
        let err = order_by
            .validate_for_message(&site)
            .expect_err("a path that is not a Site field is rejected");
        match err {
            aip::ordering::Error::UnknownField(path) => assert_eq!(path, "not_a_field"),
            other => panic!("expected UnknownField for `not_a_field`, got {other:?}"),
        }
    }
}
