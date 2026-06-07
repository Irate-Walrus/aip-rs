//! Generate the `FreightService` server stubs and message types at build time.
//!
//! Like `test-fixtures`, the protos are compiled with [`protox`] — a pure-Rust
//! protobuf compiler, so **no `protoc` is required** (ADR-0001 / ADR-0006). The
//! set is handed to [`tonic_prost_build`] to emit concrete prost message types
//! plus the async `FreightService` trait, and is *also* written to `OUT_DIR` so
//! `proto.rs` can build a runtime [`prost_reflect::DescriptorPool`].
//!
//! Per ADR-0009 the generated freight messages are **Typed messages**: each
//! derives [`prost_reflect::ReflectMessage`], so it carries its own
//! `MessageDescriptor` (resolved from the embedded pool in `proto.rs`) instead of
//! the caller looking it up by name. We take ADR-0009's recorded *fallback* path
//! rather than `prost-reflect-build`: that crate's only attribute-injecting entry
//! point (`Builder::configure`) insists on compiling the protos itself through
//! `prost_build::Config::compile_protos` (i.e. `protoc`) to mint its own
//! descriptor set — it cannot consume our externally-built `protox` set. So we
//! add the same `#[derive(ReflectMessage)]` + `#[prost_reflect(...)]` attributes
//! it would, enumerated from the `protox` FDS, keeping the build `protoc`-free.
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
    std::fs::write(
        out_dir.join("freight_descriptor_set.bin"),
        file_descriptor_set.encode_to_vec(),
    )
    .expect("write the freight descriptor set for runtime reflection");

    let mut config = tonic_prost_build::Config::new();

    // ADR-0009 Typed messages: derive `ReflectMessage` for every generated message
    // so it carries its own descriptor, resolved from the embedded
    // `crate::proto::DESCRIPTOR_POOL`. This mirrors `prost-reflect-build`'s
    // attribute loop, but sourced from the `protox` FDS we already built (see the
    // module docs). Messages that map to external types (the `prost_types`
    // well-known types) or to files we never mount have no generated type, so the
    // attribute is simply ignored — exactly as in `prost-reflect-build`.
    let pool = DescriptorPool::from_file_descriptor_set(file_descriptor_set.clone())
        .expect("the protox freight descriptor set forms a valid pool");
    for message in pool.all_messages() {
        let full_name = message.full_name();
        config
            .type_attribute(full_name, "#[derive(::prost_reflect::ReflectMessage)]")
            .type_attribute(
                full_name,
                format!(
                    "#[prost_reflect(message_name = \"{full_name}\", \
                     descriptor_pool = \"crate::proto::DESCRIPTOR_POOL\")]"
                ),
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
