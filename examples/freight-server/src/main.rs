//! A runnable tonic gRPC server demonstrating aip-rs.
//!
//! It serves einride's example `FreightService` (Shipper / Site / Shipment) over
//! an in-memory store, and grows to use each aip-rs crate as that crate's issue
//! lands. See `src/service.rs` for the per-handler `TODO(aip #N)` seams.
//!
//! Run with `cargo run -p freight-server`; it listens on `127.0.0.1:50051`.

mod proto;
mod reflect;
mod service;
mod storage;

use std::net::SocketAddr;

use tonic::transport::Server;

use crate::proto::einride::example::freight::v1::freight_service_server::FreightServiceServer;
use crate::service::FreightServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = "127.0.0.1:50051".parse()?;
    let server = FreightServer::new();

    println!("freight-server (aip-rs demo) listening on {addr}");
    Server::builder()
        .add_service(FreightServiceServer::new(server))
        .serve(addr)
        .await?;

    Ok(())
}
