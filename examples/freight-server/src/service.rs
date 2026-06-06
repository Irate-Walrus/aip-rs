//! The `FreightService` gRPC implementation — the demo's whole point.
//!
//! The Shipper standard methods (Get/List/Create/Update/Delete) are wired up as
//! the worked reference; every place an aip-rs primitive belongs is marked with
//! a `TODO(aip #N)` tied to its tracking issue, so the handlers tighten up as the
//! per-feature crates land. Site, Shipment, and the batch method return
//! `Unimplemented` until they follow the same pattern.

use std::time::{SystemTime, UNIX_EPOCH};

use aip::pagination::{PageRequest, PageToken};
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
        // Offset pagination (AIP-158) over the stable shipper listing. The
        // checksum over the request's non-pagination fields is verified by
        // `PageToken::parse`, so a token is rejected if the client changes the
        // request mid-pagination; an empty token starts at offset 0.
        let checksum = request_checksum_of("einride.example.freight.v1.ListShippersRequest", &req)?;
        let current = PageToken::parse(&req, checksum)
            .map_err(|e| Status::invalid_argument(format!("invalid page_token: {e}")))?;
        let page_size = effective_page_size(req.page_size());
        let mut shippers = self.storage.list_shippers();
        let total = shippers.len();
        let start = usize::try_from(current.offset).unwrap_or(0).min(total);
        let end = start.saturating_add(page_size).min(total);
        // Only hand back a `next_page_token` when results remain past this page.
        let next_page_token = if end < total {
            current.next(page_size as i32).encode()
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
        _: Request<ListSitesRequest>,
    ) -> Result<Response<ListSitesResponse>, Status> {
        // TODO(aip #11): apply the AIP-160 filter here once ListSites is wired.
        // The comparison pipeline (`aip::filtering`: a Declarations allowlist →
        // `check` → a native AST to walk) is ready; this seam is blocked on
        // implementing the method (Site storage + AIP-132 ordering, #9/#10) and
        // on `ListSitesRequest` gaining a `filter` field — the vendored einride
        // proto carries none yet. Operator/function/enum coverage grows with
        // #12–#15.
        Err(unimplemented("ListSites"))
    }

    async fn create_site(&self, _: Request<CreateSiteRequest>) -> Result<Response<Site>, Status> {
        Err(unimplemented("CreateSite"))
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

/// Computes [`aip::pagination::request_checksum`] for a concrete request.
///
/// The library's reflective surface is `DynamicMessage`-based (ADR-0001), but the
/// generated request types carry no reflection. We transcode the request to a
/// `DynamicMessage` via the [`reflect`] bridge, then checksum it. `full_name` is
/// the request's fully-qualified message name.
fn request_checksum_of<M: prost::Message>(full_name: &str, request: &M) -> Result<u32, Status> {
    let dynamic = reflect::to_dynamic(&reflect::descriptor(full_name), request);
    aip::pagination::request_checksum(&dynamic)
        .map_err(|e| Status::internal(format!("compute request checksum: {e}")))
}

/// Resolves the effective page size from a request's `page_size`, applying the
/// AIP-158 default (when unset or non-positive) and the [`MAX_PAGE_SIZE`] cap.
fn effective_page_size(requested: i32) -> usize {
    if requested <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        (requested as usize).min(MAX_PAGE_SIZE)
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
