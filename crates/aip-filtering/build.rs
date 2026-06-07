//! Generate the CEL `google.api.expr.v1alpha1` types for the `cel-proto` feature.
//!
//! This runs only when `CARGO_FEATURE_CEL_PROTO` is set, so a default build does
//! no proto codegen and the generated types never reach the crate (the
//! `cel_proto` module is itself `#[cfg(feature = "cel-proto")]`). Like the rest
//! of the workspace, the vendored `.proto` are compiled with [`protox`] — a
//! pure-Rust protobuf compiler, so **no `protoc` is required** (ADR-0001) — and
//! the resulting descriptor set is handed to [`prost_build`] via `compile_fds`,
//! which never shells out to `protoc` either.
//!
//! The well-known types the CEL protos import (`google/protobuf/*`) are supplied
//! by `protox` and mapped to [`prost_types`] by `prost-build`, so only the
//! `google.api.expr.v1alpha1.rs` module is emitted for `src/lib.rs` to include.

use std::{env, path::PathBuf};

/// The CEL `.proto` files to generate, relative to this crate's `proto/` root.
/// Their imports (the sibling `syntax.proto` and the well-known types) are
/// resolved by `protox` from the include path.
const ROOT_PROTOS: &[&str] = &[
    "google/api/expr/v1alpha1/syntax.proto",
    "google/api/expr/v1alpha1/checked.proto",
];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));
    let proto_root = manifest_dir.join("proto");

    // Recompile whenever a vendored proto changes, even on builds where the
    // feature is off (so toggling it on picks up edits).
    println!("cargo:rerun-if-changed={}", proto_root.display());

    // Off by default: skip all codegen unless the `cel-proto` feature is on.
    if env::var_os("CARGO_FEATURE_CEL_PROTO").is_none() {
        return;
    }

    let file_descriptor_set = protox::compile(
        ROOT_PROTOS.iter().map(|p| proto_root.join(p)),
        [&proto_root],
    )
    .expect("protox failed to compile the vendored CEL v1alpha1 protos");

    prost_build::Config::new()
        // Drop the proto comments: some CEL doc comments (e.g. `Comprehension`,
        // `Decl`) carry ```-fenced pseudocode that rustdoc would otherwise try to
        // run as Rust doc-tests on the `include!`d output and fail to compile.
        .disable_comments(["."])
        .compile_fds(file_descriptor_set)
        .expect("prost-build failed to generate the CEL v1alpha1 types");
}
