//! A runnable tonic gRPC server demonstrating aip-rs.
//!
//! It serves einride's example `FreightService` (Shipper / Site / Shipment) and
//! the `google.iam.v1.IAMPolicy` service over an in-memory store, and grows to
//! use each aip-rs crate as that crate's issue lands. See `src/service.rs` for
//! the per-handler `TODO(aip #N)` seams.
//!
//! Run with `cargo run -p freight-server`; it listens on `127.0.0.1:50051`.

mod iam;
mod proto;
mod service;
mod storage;

use std::net::SocketAddr;

use tonic::transport::Server;

use crate::iam::IamServer;
use crate::proto::einride::example::freight::v1::freight_service_server::FreightServiceServer;
use crate::proto::google::iam::v1::iam_policy_server::IamPolicyServer;
use crate::service::FreightServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:50051".parse()?;
    let server = FreightServer::new();
    // The IAM tracer bullet (aip #64): the `google.iam.v1.IAMPolicy` service over
    // its own resource-name-keyed policy store, served alongside `FreightService`.
    let iam = IamServer::new();

    println!("freight-server (aip-rs demo) listening on {addr}");
    Server::builder()
        .add_service(FreightServiceServer::new(server))
        .add_service(IamPolicyServer::new(iam))
        .serve(addr)
        .await?;

    Ok(())
}
