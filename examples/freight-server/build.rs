//! Generate the `FreightService` server stubs and message types at build time.
//!
//! Like `test-fixtures`, the protos are compiled with [`protox`] — a pure-Rust
//! protobuf compiler, so **no `protoc` is required** (ADR-0001). The difference
//! is the sink: `test-fixtures` embeds the raw `FileDescriptorSet` for
//! reflection, whereas here the set is handed to [`tonic_prost_build`] to emit
//! concrete prost message types plus the async `FreightService` trait.
//!
//! The example vendors its own copy of the freight protos and their googleapis
//! imports under `proto/`, so it builds standalone without reaching into another
//! crate's source.
//!
//! The descriptor set is also written to `OUT_DIR` so `proto.rs` can build a
//! runtime [`prost_reflect::DescriptorPool`] — needed to transcode a concrete
//! request into a `DynamicMessage` for the reflective
//! `aip::pagination::request_checksum`.

use std::{env, path::PathBuf};

use prost::Message as _;

/// The freight `.proto` files to serve, relative to this crate's `proto/` root.
/// Their imports (sibling freight protos, vendored `google/api` + `google/type`,
/// and the well-known types) are resolved by `protox` from the include path.
const ROOT_PROTOS: &[&str] = &[
    "einride/example/freight/v1/freight_service.proto",
    "einride/example/freight/v1/shipment.proto",
    "einride/example/freight/v1/shipper.proto",
    "einride/example/freight/v1/site.proto",
];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));

    // This crate's vendored proto tree (freight + googleapis imports).
    let proto_root = manifest_dir.join("proto");

    let file_descriptor_set = protox::compile(
        ROOT_PROTOS.iter().map(|p| proto_root.join(p)),
        [&proto_root],
    )
    .expect("protox failed to compile the freight protos");

    // Persist the descriptor set so `proto.rs` can build a runtime reflection
    // pool. Written before `compile_fds` consumes the set.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    std::fs::write(
        out_dir.join("freight_descriptor_set.bin"),
        file_descriptor_set.encode_to_vec(),
    )
    .expect("write the freight descriptor set for runtime reflection");

    // Server-only: we implement the service, we don't call it.
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_fds(file_descriptor_set)
        .expect("tonic-prost-build failed to generate the freight service");

    // Recompile whenever any shared proto changes.
    println!("cargo:rerun-if-changed={}", proto_root.display());
}
