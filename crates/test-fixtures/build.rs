//! Compile the vendored example protos into a `FileDescriptorSet` with
//! [`protox`] — a pure-Rust protobuf compiler, so **no `protoc` is required**.
//!
//! The encoded set is written to `$OUT_DIR/file_descriptor_set.bin` and embedded
//! by `src/lib.rs` via `include_bytes!`. The `proto/` directory is the single
//! include path: it holds the human-readable einride example sources *and* the
//! vendored `google/api` + `google/type` imports. The `google/protobuf/*`
//! well-known types are supplied by `protox` itself.
//!
//! # Why `encode_file_descriptor_set` not `compile`
//!
//! `protox::compile` returns `prost_types::FileDescriptorSet`, which cannot
//! represent extension fields (such as `google.api.field_behavior`) — prost
//! drops unknown/extension bytes when it decodes into a generated struct.
//! `Compiler::encode_file_descriptor_set` encodes from the internal
//! prost_reflect pool, which preserves all extension bytes, so the resulting
//! binary correctly contains field 1052 and similar annotations.

use std::{env, path::PathBuf};

/// The example `.proto` files under test, relative to `proto/`. Their imports
/// (sibling einride protos, vendored googleapis protos, and the well-known
/// types) are resolved by `protox` from the include path.
const ROOT_PROTOS: &[&str] = &[
    "einride/example/freight/v1/freight_service.proto",
    "einride/example/freight/v1/shipment.proto",
    "einride/example/freight/v1/shipper.proto",
    "einride/example/freight/v1/site.proto",
    "einride/example/syntax/v1/syntax.proto",
    "einride/example/syntax/v1/fieldbehaviors.proto",
];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));
    let proto_root = manifest_dir.join("proto");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));

    let bytes = protox::Compiler::new([&proto_root])
        .expect("protox failed to initialise with the proto include path")
        .include_imports(true)
        .include_source_info(true)
        .open_files(ROOT_PROTOS.iter().map(|p| proto_root.join(p)))
        .expect("protox failed to compile the vendored example protos")
        .encode_file_descriptor_set();

    let out_path = out_dir.join("file_descriptor_set.bin");
    std::fs::write(&out_path, bytes).expect("failed to write file descriptor set");

    // Recompile whenever any vendored proto changes.
    println!("cargo:rerun-if-changed={}", proto_root.display());
}
