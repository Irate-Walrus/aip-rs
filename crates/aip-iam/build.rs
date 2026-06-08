//! Generate the `google.iam.v1.Policy` / `Binding` types for the `iam-proto`
//! feature.
//!
//! This runs only when `CARGO_FEATURE_IAM_PROTO` is set, so a default build does
//! no proto codegen and the generated types never reach the crate (the `proto`
//! module is itself `#[cfg(feature = "iam-proto")]`). Like the rest of the
//! workspace, the vendored `.proto` are compiled with [`protox`] — a pure-Rust
//! protobuf compiler, so **no `protoc` is required** (ADR-0001) — and the
//! resulting descriptor set is handed to [`prost_build`] via `compile_fds`,
//! which never shells out to `protoc` either. This mirrors `aip-filtering`'s
//! `cel-proto` build.
//!
//! Only `policy.proto` is a root: it pulls in `google/type/expr.proto` (the
//! `Binding.condition`) as an import, so both packages — `google.iam.v1.rs` and
//! `google.r#type.rs` — are emitted for `src/lib.rs` to include.

use std::{env, path::PathBuf};

/// The IAM `.proto` files to generate, relative to this crate's `proto/` root.
/// Their imports (the `google/type/expr.proto` `Binding.condition` carries) are
/// resolved by `protox` from the include path.
const ROOT_PROTOS: &[&str] = &["google/iam/v1/policy.proto"];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));
    let proto_root = manifest_dir.join("proto");

    // Recompile whenever a vendored proto changes, even on builds where the
    // feature is off (so toggling it on picks up edits).
    println!("cargo:rerun-if-changed={}", proto_root.display());

    // Off by default: skip all codegen unless the `iam-proto` feature is on.
    if env::var_os("CARGO_FEATURE_IAM_PROTO").is_none() {
        return;
    }

    let file_descriptor_set = protox::compile(
        ROOT_PROTOS.iter().map(|p| proto_root.join(p)),
        [&proto_root],
    )
    .expect("protox failed to compile the vendored google.iam.v1 protos");

    prost_build::Config::new()
        // Drop the proto comments: the vendored `policy.proto` carries ```-fenced
        // JSON/YAML examples in its doc comments that rustdoc would otherwise try
        // to run as Rust doc-tests on the `include!`d output and fail to compile.
        .disable_comments(["."])
        .compile_fds(file_descriptor_set)
        .expect("prost-build failed to generate the google.iam.v1 types");
}
