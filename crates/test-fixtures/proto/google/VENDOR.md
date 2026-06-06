# Vendored googleapis protos

These `google/api/*` and `google/type/*` files are the dependency protos
imported by the einride example protos (`einride/example/freight` and
`einride/example/syntax`). They are vendored verbatim so the fixture harness
compiles with [`protox`](https://crates.io/crates/protox) — **no `protoc`,
no `buf`, no network at build time**.

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
  - google/type/latlng.proto
