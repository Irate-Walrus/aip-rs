//! Compile the vendored example protos into a `FileDescriptorSet` with
//! [`protox`] — a pure-Rust protobuf compiler, so **no `protoc` is required**.
//!
//! The encoded set is written to `$OUT_DIR/file_descriptor_set.bin` and embedded
//! by `src/lib.rs` via `include_bytes!`. The `proto/` directory is the single
//! include path: it holds the human-readable einride example sources *and* the
//! vendored `google/api` + `google/type` imports. The `google/protobuf/*`
//! well-known types are supplied by `protox` itself.

use std::{env, path::PathBuf};

use prost::Message;

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

    let file_descriptor_set = protox::compile(
        ROOT_PROTOS.iter().map(|p| proto_root.join(p)),
        [&proto_root],
    )
    .expect("protox failed to compile the vendored example protos");

    let out_path = out_dir.join("file_descriptor_set.bin");
    std::fs::write(&out_path, file_descriptor_set.encode_to_vec())
        .expect("failed to write file descriptor set");

    // Recompile whenever any vendored proto changes.
    println!("cargo:rerun-if-changed={}", proto_root.display());
}
