//! The `FreightService` gRPC implementation — the demo's whole point.
//!
//! The Shipper standard methods (Get/List/Create/Update/Delete) are wired up as
//! the worked reference; every place an aip-rs primitive belongs is marked with
//! a `TODO(aip #N)` tied to its tracking issue, so the handlers tighten up as the
//! per-feature crates land. Site, Shipment, and the batch method return
//! `Unimplemented` until they follow the same pattern.

use std::time::{SystemTime, UNIX_EPOCH};

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
        _request: Request<ListShippersRequest>,
    ) -> Result<Response<ListShippersResponse>, Status> {
        // TODO(aip #6/#7): honour `page_size`/`page_token` with `aip::pagination`
        // instead of returning every shipper in a single unpaged response.
        let shippers = self.storage.list_shippers();
        Ok(Response::new(ListShippersResponse {
            shippers,
            next_page_token: String::new(),
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
        // TODO(aip #5): assign the resource ID with `aip::resourceid::generate`
        // (or validate a user-supplied one) instead of a bare counter.
        let id = self.storage.next_id();
        // Format the canonical resource name `shippers/{shipper}` from its
        // pattern (AIP-122) rather than hand-concatenating the segments.
        let shipper_pattern = format!("{SHIPPERS_COLLECTION}/{{shipper}}");
        shipper.name = aip::resourcename::Pattern::parse(&shipper_pattern)
            .expect("the shipper collection pattern is valid")
            .format([("shipper", id.to_string().as_str())])
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
        let mut shipper = req
            .shipper
            .ok_or_else(|| Status::invalid_argument("shipper is required"))?;
        let existing = self
            .storage
            .get_shipper(&shipper.name)
            .ok_or_else(|| Status::not_found(format!("shipper `{}` not found", shipper.name)))?;
        // TODO(aip #8): apply `req.update_mask` with `aip::fieldmask::update` so a
        // partial mask touches only the named fields. For now this is a full
        // replacement that preserves the server-owned `create_time`.
        let _ = &req.update_mask;
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
