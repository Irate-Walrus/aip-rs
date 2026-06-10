//! A runnable tonic gRPC server demonstrating aip-rs.
//!
//! It serves einride's example `FreightService` (Shipper / Site / Shipment) and
//! the `google.iam.v1.IAMPolicy` service over an in-memory store, and grows to
//! use each aip-rs crate as that crate's issue lands. See `src/service.rs` for
//! the per-handler `TODO(aip #N)` seams.
//!
//! Run with `cargo run -p freight-server`; it listens on `127.0.0.1:50051`.

mod iam;
#[cfg(test)]
mod integration_tests;
mod proto;
mod service;
mod storage;

use std::net::SocketAddr;
use std::sync::Arc;

use tonic::transport::Server;
use tonic_reflection::server::Builder as ReflectionBuilder;

use crate::iam::IamServer;
use crate::proto::einride::example::freight::v1::freight_service_server::FreightServiceServer;
use crate::proto::google::iam::v1::iam_policy_server::IamPolicyServer;
use crate::proto::FILE_DESCRIPTOR_SET;
use crate::service::FreightServer;
use crate::storage::PolicyStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:50051".parse()?;
    // One resource-name-keyed policy store shared by both services (aip #64, #67):
    // `IAMPolicy` mutates the Policies, and `FreightService` reads them to make the
    // AIP-211 authorization decision its handlers gate on. So a Policy set via
    // `SetIamPolicy` actually governs who may `GetShipper`.
    let policies = Arc::new(PolicyStore::new());
    let server = FreightServer::with_policies(Arc::clone(&policies));
    // The `google.iam.v1.IAMPolicy` service over that shared store (aip #64),
    // served alongside `FreightService`.
    let iam = IamServer::with_store(policies);
    // gRPC server reflection (issue #91): feeds the same FileDescriptorSet the
    // runtime DescriptorPool uses so grpcurl works without `-import-path`/`-proto`.
    // Both v1 and v1alpha are served: grpcurl >= 1.8 speaks v1; older grpcurl and
    // some other tools (evans, ghz) speak only v1alpha. The Builder is consuming,
    // so two configure() chains are needed.
    let reflection_v1 = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let reflection_v1alpha = ReflectionBuilder::configure()
        .register_encoded_file_descriptor_set(FILE_DESCRIPTOR_SET)
        .build_v1alpha()?;

    println!("freight-server (aip-rs demo) listening on {addr}");
    Server::builder()
        .add_service(FreightServiceServer::new(server))
        .add_service(IamPolicyServer::new(iam))
        .add_service(reflection_v1)
        .add_service(reflection_v1alpha)
        .serve(addr)
        .await?;

    Ok(())
}
