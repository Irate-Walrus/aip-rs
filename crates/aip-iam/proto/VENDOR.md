# Vendored googleapis protos

These `google/iam/v1/*` and `google/type/*` files are the source for the
`google.iam.v1.Policy` / `Binding` structure generated behind the optional
`iam-proto` feature (ADR-0010). They are vendored verbatim so the build
compiles them with [`protox`](https://crates.io/crates/protox) — **no `protoc`,
no `buf`, no network at build time** (ADR-0001).

Only the structural `policy.proto` is needed here: `aip-iam` owns the Policy
*value*, while the `google.iam.v1.IAMPolicy` *service* (and its request
messages) is vendored and served by the example (`examples/freight-server`),
which keeps its own proto copy per its standalone-build convention.

The `google/protobuf/*` well-known types are **not** vendored: `protox` bundles
them.

Source: https://github.com/googleapis/googleapis

Files:
  - google/iam/v1/policy.proto
  - google/type/expr.proto      (imported by policy.proto for Binding.condition)
