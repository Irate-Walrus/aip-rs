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
//!
//! # Why `encode_file_descriptor_set` for the runtime binary
//!
//! `Compiler::file_descriptor_set()` returns a `prost_types::FileDescriptorSet`,
//! which loses extension-field bytes (e.g. `google.api.field_behavior = 1052`)
//! because prost-generated structs discard unknown/extension fields. The runtime
//! `DescriptorPool` must see those bytes so that `field.options()` returns the
//! annotations. `encode_file_descriptor_set()` encodes from the internal
//! prost_reflect pool and preserves all extension bytes.

use std::{env, path::PathBuf};

/// The `.proto` files to serve, relative to this crate's `proto/` root. Their
/// imports (sibling freight protos, vendored `google/api` + `google/type` +
/// `google/iam/v1`, and the well-known types) are resolved by `protox` from the
/// include path. `iam_policy.proto` adds the `google.iam.v1.IAMPolicy` service
/// (aip #64) and pulls in `policy.proto` / `options.proto` / `expr.proto`.
const ROOT_PROTOS: &[&str] = &[
    "einride/example/freight/v1/freight_service.proto",
    "einride/example/freight/v1/shipment.proto",
    "einride/example/freight/v1/shipper.proto",
    "einride/example/freight/v1/site.proto",
    "google/iam/v1/iam_policy.proto",
];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"));

    // This crate's vendored proto tree (freight + googleapis imports).
    let proto_root = manifest_dir.join("proto");

    let mut compiler = protox::Compiler::new([&proto_root])
        .expect("protox failed to initialise with the proto include path");
    compiler
        .include_imports(true)
        .include_source_info(true)
        .open_files(ROOT_PROTOS.iter().map(|p| proto_root.join(p)))
        .expect("protox failed to compile the freight protos");

    // Encode from the internal pool so extension bytes (e.g. field_behavior, tag
    // 1052) are preserved in the runtime descriptor set.
    let raw_bytes = compiler.encode_file_descriptor_set();

    // Persist the descriptor set so `proto.rs` can build a runtime reflection
    // pool with full extension support.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    std::fs::write(out_dir.join("freight_descriptor_set.bin"), &raw_bytes)
        .expect("write the freight descriptor set for runtime reflection");

    // The prost_types version (loses extensions) is only needed for code
    // generation; tonic_prost_build doesn't need to read extension values.
    let file_descriptor_set = compiler.file_descriptor_set();

    let mut config = tonic_prost_build::Config::new();

    // Share the `google.iam.v1` *message* layer with `aip::iam::proto` (aip #65):
    // map the Policy structure (and the `google.type.Expr` condition) onto the
    // crate's generated types so the `IAMPolicy` service we still generate locally
    // uses the very `Policy` / `Binding` the structural read-modify-write helpers
    // operate on — one type, not a structurally-identical duplicate. Only these
    // messages are externed; the service trait and its request types
    // (`SetIamPolicyRequest`, `GetPolicyOptions`, …) are still generated here, and
    // `google.type.LatLng` (the freight `Site` location) stays local.
    for (proto_path, rust_path) in [
        (
            ".google.iam.v1.Policy",
            "::aip::iam::proto::google::iam::v1::Policy",
        ),
        (
            ".google.iam.v1.Binding",
            "::aip::iam::proto::google::iam::v1::Binding",
        ),
        (
            ".google.iam.v1.AuditConfig",
            "::aip::iam::proto::google::iam::v1::AuditConfig",
        ),
        (
            ".google.iam.v1.AuditLogConfig",
            "::aip::iam::proto::google::iam::v1::AuditLogConfig",
        ),
        (
            ".google.iam.v1.PolicyDelta",
            "::aip::iam::proto::google::iam::v1::PolicyDelta",
        ),
        (
            ".google.iam.v1.BindingDelta",
            "::aip::iam::proto::google::iam::v1::BindingDelta",
        ),
        (
            ".google.iam.v1.AuditConfigDelta",
            "::aip::iam::proto::google::iam::v1::AuditConfigDelta",
        ),
        (
            ".google.type.Expr",
            "::aip::iam::proto::google::r#type::Expr",
        ),
    ] {
        config.extern_path(proto_path, rust_path);
    }

    // ADR-0009 Typed messages: derive `ReflectMessage` for every generated message
    // so it carries its own descriptor, resolved from the embedded
    // `crate::proto::DESCRIPTOR_POOL`. This mirrors `prost-reflect-build`'s
    // attribute loop, but sourced from the `protox` FDS we already built (see the
    // module docs). Messages that map to external types (the `prost_types`
    // well-known types) or to files we never mount have no generated type, so the
    // attribute is simply ignored — exactly as in `prost-reflect-build`.
    //
    // Use the Compiler's internal DescriptorPool (has extension bytes) for
    // message enumeration — both produce the same set of names.
    let pool = compiler.descriptor_pool();
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
