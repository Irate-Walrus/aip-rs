# Vendored googleapis protos

These `google/api/*`, `google/type/*`, and `google/iam/v1/*` files are the
dependency protos imported by the einride example protos
(`einride/example/freight` and `einride/example/syntax`) and the
`google.iam.v1.IAMPolicy` service the demo serves (aip #64). They are vendored
verbatim so the example compiles with
[`protox`](https://crates.io/crates/protox) — **no `protoc`, no `buf`, no
network at build time**.

The `google/iam/v1/*` set defines the `IAMPolicy` service the example serves
(`GetIamPolicy` / `SetIamPolicy` / `TestIamPermissions`) and the `Policy` /
`Binding` structure it stores. The example keeps its own proto copy rather than
reaching into `aip-iam` (which generates the same `Policy` / `Binding` behind its
`iam-proto` feature), matching the standalone-build convention below.

The `google/protobuf/*` well-known types (descriptor, timestamp, duration,
field_mask) are **not** vendored here: `protox` bundles them.

Source: https://github.com/googleapis/googleapis
Commit: ff15be54722218705740b9fc6223d264c4cdb6dd

Files:
  - google/api/annotations.proto
  - google/api/client.proto
  - google/api/field_behavior.proto
  - google/api/http.proto
  - google/api/launch_stage.proto
  - google/api/resource.proto
  - google/iam/v1/iam_policy.proto
  - google/iam/v1/options.proto
  - google/iam/v1/policy.proto
  - google/type/expr.proto
  - google/type/latlng.proto
