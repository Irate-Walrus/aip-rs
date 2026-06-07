//! Generate the `FreightService` server stubs and message types at build time.
//!
//! Like `test-fixtures`, the protos are compiled with [`protox`] — a pure-Rust
//! protobuf compiler, so **no `protoc` is required** (ADR-0001). The set is
//! handed to [`tonic_prost_build`] to emit concrete prost message types plus the
//! async `FreightService` trait, and is *also* written to `OUT_DIR` so `proto.rs`
//! can build a runtime [`prost_reflect::DescriptorPool`] — needed to transcode a
//! concrete request/message into a `DynamicMessage` for the reflective primitives
//! (`aip::pagination::request_checksum`, `aip::fieldmask`).
//!
//! The example vendors its own copy of the freight protos and their googleapis
//! imports under `proto/`, so it builds standalone without reaching into another
//! crate's source.

use std::{env, path::PathBuf};

use prost::Message as _;
use prost_reflect::DescriptorPool;

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
    // pool. Written before `compile_fds_with_config` consumes the set.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let descriptor_set_bytes = file_descriptor_set.encode_to_vec();
    std::fs::write(
        out_dir.join("freight_descriptor_set.bin"),
        &descriptor_set_bytes,
    )
    .expect("write the freight descriptor set for runtime reflection");

    // Emit `prost::Name` impls for the generated messages so a handler can derive
    // a request's fully-qualified name from its type (`M::full_name()`) rather than
    // hand-typing it — see `request_checksum_of` in `service.rs`.
    let mut config = tonic_prost_build::Config::new();
    config.enable_type_names();

    // Make every generated message a Typed message (ADR-0009): derive
    // `prost_reflect::ReflectMessage` so the value carries its own Descriptor.
    // `prost-reflect-build` automates this, but its only FDS-consuming entry
    // points re-run `protoc` — we already have a `protox`-built descriptor set
    // (no `protoc`, ADR-0001/0006), so we replicate its attribute wiring against
    // it: enumerate the messages and point the derive at the existing runtime
    // pool (`crate::proto::DESCRIPTOR_POOL`). Messages with no generated Rust
    // type — the well-known `google.protobuf.*`, options-only `google.api.*`, and
    // synthetic map-entry messages — match nothing, so their attributes are
    // harmless no-ops.
    let pool = DescriptorPool::decode(descriptor_set_bytes.as_slice())
        .expect("the protox descriptor set decodes into a reflection pool");
    for message in pool.all_messages() {
        let full_name = message.full_name();
        config
            .type_attribute(full_name, "#[derive(::prost_reflect::ReflectMessage)]")
            .type_attribute(
                full_name,
                format!(r#"#[prost_reflect(message_name = "{full_name}")]"#),
            )
            .type_attribute(
                full_name,
                r#"#[prost_reflect(descriptor_pool = "crate::proto::DESCRIPTOR_POOL")]"#,
            );
    }

    // Server-only: we implement the service, we don't call it.
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_fds_with_config(file_descriptor_set, config)
        .expect("tonic-prost-build failed to generate the freight service");

    // Recompile whenever any shared proto changes.
    println!("cargo:rerun-if-changed={}", proto_root.display());
}
