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
use prost_reflect::ReflectMessage;
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
use crate::storage::Storage;

/// The shipper collection ID — the root segment of every shipper resource name.
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
        validate_shipper_name("name", &name)?;
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
            return Err(required_field("shipper.display_name"));
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
        validate_shipper_name("name", &name)?;
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

        // The AIP-160 `filter` is applied first, at the source: parse +
        // type-check it (`aip::filtering`), transpile it to a parameterized
        // `Predicate` (`aip_sql`), and let the SQLite-backed store run it (#39). An
        // empty filter lists every site.
        let predicate = if req.filter.is_empty() {
            None
        } else {
            // An invalid/unsupported filter converts to `INVALID_ARGUMENT`:
            // `check` via `aip-filtering`'s AIP-193 `From<Error>` (#16), and a
            // construct beyond this slice's `=`/`AND` transpiler explicitly.
            let filter = aip::filtering::check(&req.filter, &site_declarations())?;
            let predicate = aip_sql::transpile_filter(&filter, &site_schema())
                .map_err(|e| Status::invalid_argument(format!("filter: {e}")))?;
            Some(predicate)
        };

        // Sites under this parent, in the store's stable resource-name order, then
        // sorted by `order_by`. The sort is stable, so the name order breaks ties
        // and the ordering stays consistent across pages.
        let mut sites: Vec<Site> = self
            .storage
            .list_sites_matching(predicate.as_ref())
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
        validate_shipper_name("parent", &req.parent)?;
        let mut site = req
            .site
            .ok_or_else(|| Status::invalid_argument("site is required"))?;
        if site.display_name.is_empty() {
            return Err(required_field("site.display_name"));
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

/// The AIP-160 declarations a `ListSites` filter is checked against: the scalar
/// string columns `display_name` and `name`, with `=` and `AND`. Deliberately
/// narrower than `standard_functions` so the checker rejects anything this
/// slice's transpiler can't lower (e.g. `OR`, `!=`) with a clean
/// `INVALID_ARGUMENT`, rather than failing later in transpile.
fn site_declarations() -> aip::filtering::Declarations {
    use aip::filtering::{function, Overload, Type};
    aip::filtering::Declarations::builder()
        .ident("display_name", Type::String)
        .ident("name", Type::String)
        .function(
            function::EQUALS,
            vec![Overload::new(Type::Bool, vec![Type::String, Type::String])],
        )
        .function(
            function::AND,
            vec![Overload::new(Type::Bool, vec![Type::Bool, Type::Bool])],
        )
        .build()
        .expect("site filter declarations are valid")
}

/// Maps the filterable Site identifiers onto their SQLite columns (#39). Both map
/// to identically-named columns in the `sites` table.
fn site_schema() -> aip_sql::Schema {
    aip_sql::Schema::builder()
        .column("display_name", "display_name")
        .column("name", "name")
        .build()
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
/// valid resource name that matches the `shippers/{shipper}` pattern. Returns
/// `INVALID_ARGUMENT` otherwise. `field` is the request field the value came from
/// (`name` or `parent`), so the AIP-193 `BadRequest` points at the right one.
fn validate_shipper_name(field: &str, value: &str) -> Result<(), Status> {
    // A malformed name converts via the crate's AIP-193 `From<Error> for Status`
    // (#16) to an `INVALID_ARGUMENT` carrying an `ErrorInfo`. The collection-match
    // check below is the server's own policy, so it builds its own AIP-193 details.
    aip::resourcename::validate(value)?;
    let pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}");
    if !aip::resourcename::is_match(&pattern, value) {
        return Err(field_violation(
            field,
            format!("must match the pattern `{pattern}`"),
            "RESOURCE_NAME_PATTERN_MISMATCH",
        ));
    }
    Ok(())
}

/// The AIP-193 `ErrorInfo.domain` for errors the server raises itself — the
/// presence and policy checks no aip-rs primitive covers. The aip-rs crates use
/// their own (`aip-rs`) domain for the values they validate (#16).
const SERVICE_DOMAIN: &str = "freight.example.com";

/// Builds an `INVALID_ARGUMENT` carrying AIP-193 standard details for one bad
/// request field: a `BadRequest` field violation plus the mandatory `ErrorInfo`
/// (with the `field` echoed in `metadata`, since it appears in the message). This
/// mirrors the shape the aip-rs crates emit via `From<Error>` (#16) for the
/// server's own validations, which no primitive covers.
fn field_violation(field: &str, description: impl Into<String>, reason: &str) -> Status {
    use std::collections::HashMap;
    use tonic_types::{ErrorDetails, StatusExt};

    let description = description.into();
    let mut details = ErrorDetails::with_bad_request_violation(field, description.clone());
    details.set_error_info(
        reason,
        SERVICE_DOMAIN,
        HashMap::from([("field".to_owned(), field.to_owned())]),
    );
    Status::with_error_details(
        tonic::Code::InvalidArgument,
        format!("{field}: {description}"),
        details,
    )
}

/// A [`field_violation`] for a required request field left empty.
fn required_field(field: &str) -> Status {
    field_violation(field, "field is required", "FIELD_REQUIRED")
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

    #[test]
    fn sortable_site_paths_resolve_on_the_site_descriptor() {
        // `ListSites` gates `order_by` with the curated `validate_for_paths`
        // allow-list (#9), since the in-memory `sort_sites` only knows those
        // paths. `validate_for_message` (#10) guards the allow-list itself: every
        // sortable path must be a real `Site` field, so the allow-list can't
        // silently drift from the proto. The `Site` descriptor comes straight off
        // the Typed message (ADR-0009), no by-name pool lookup.
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
            }))
            .await
            .expect("create_shipper succeeds")
            .into_inner()
    }

    /// Applies an `UpdateShipper` with the given incoming shipper and mask,
    /// returning the updated resource.
    async fn update_shipper(server: &FreightServer, shipper: Shipper, mask: &[&str]) -> Shipper {
        server
            .update_shipper(Request::new(UpdateShipperRequest {
                shipper: Some(shipper),
                update_mask: Some(field_mask(mask)),
            }))
            .await
            .expect("update_shipper succeeds")
            .into_inner()
    }

    #[tokio::test]
    async fn update_shipper_applies_update_mask_via_typed_facade() {
        // Exercises the typed `update` facade (#48) end-to-end through the handler:
        // a masked field changes, an unmasked field is untouched, and a masked
        // field absent from the request is cleared.
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

        // (3) A masked path absent from the request clears that field.
        let cleared = update_shipper(
            &server,
            Shipper {
                name: name.clone(),
                ..Default::default()
            },
            &["display_name"],
        )
        .await;
        assert_eq!(cleared.display_name, "");
    }

    #[tokio::test]
    async fn create_shipper_missing_display_name_carries_aip193_details() {
        use tonic_types::StatusExt as _;

        // The server's own presence check (no aip-rs primitive covers it) still
        // emits AIP-193 details: a `BadRequest` naming the field plus an
        // `ErrorInfo`.
        let server = FreightServer::new();
        let status = server
            .create_shipper(Request::new(CreateShipperRequest {
                shipper: Some(Shipper::default()),
            }))
            .await
            .expect_err("an empty display_name is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached");
        assert_eq!(bad.field_violations.len(), 1);
        assert_eq!(bad.field_violations[0].field, "shipper.display_name");

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193 MUST)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, SERVICE_DOMAIN);
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
    async fn filter_beyond_the_slice_is_invalid_argument() {
        // `OR` is outside this slice's declarations, so the checker rejects it with
        // `INVALID_ARGUMENT` before it ever reaches the transpiler.
        let server = FreightServer::new();
        seed_site(&server, "Alpha", 0.0).await;
        let status = server
            .list_sites(Request::new(ListSitesRequest {
                parent: PARENT.to_owned(),
                filter: r#"display_name = "Alpha" OR display_name = "Bravo""#.to_owned(),
                ..Default::default()
            }))
            .await
            .expect_err("an unsupported filter is rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }
}
