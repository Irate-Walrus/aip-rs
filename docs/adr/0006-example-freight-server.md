# Demo with a runnable freight gRPC server, grown issue by issue

aip-rs needs an executable end-to-end demo: a place to see the primitives used
together (resource names → IDs → pagination → field masks → filtering →
ordering → errors) and an integration-test surface the per-crate unit tests
can't provide. `test-fixtures` does **not** fill this role — it is a test-only
reflection harness (a `DescriptorPool` + JSON→`DynamicMessage`), with no
generated service types, no storage, and nothing to run.

We add `examples/freight-server`: a tonic gRPC server implementing einride's
example `FreightService` (Shipper / Site / Shipment) over an in-memory store,
mirroring `aip-go`'s `examples/examplelibrary`. It is a **living demo** — it
compiles from day one with the Shipper standard methods as the worked reference,
and each handler carries a `TODO(aip #N)` seam where a primitive plugs in as its
issue lands. The unimplemented methods return gRPC `Unimplemented`.

## Decisions

- **gRPC over the einride freight protos.** The freight methods map 1:1 onto the
  primitives (List → pagination/ordering/filtering, Update → field mask, names →
  resourcename/resourceid), so the demo exercises the whole SDK. The example
  vendors its own copy of the freight protos and their googleapis imports under
  `proto/`, so it builds standalone — a copy of the same einride sources
  `test-fixtures` uses, not a shared tree.
- **No `protoc`.** `build.rs` compiles with `protox` and feeds the
  `FileDescriptorSet` to `tonic-prost-build`, keeping the pure-Rust-build
  property of ADR-0001.
- **In-memory, database-agnostic storage.** Just keyed maps (ADR-0005), so the
  gRPC layer — not a datastore — is what exercises the primitives.
- **A workspace member, never published.** Lives under `examples/*` (added to the
  workspace) and is `publish = false`; its server-only deps (`tokio`,
  `tonic-prost`, `tonic-prost-build`) stay out of the core crates.

## Considered Options

- **HTTP/REST via axum** using the `google.api.http` annotations — browser- and
  curl-friendly, but needs hand-written transcoding the protos define for gRPC.
  Rejected for v0.1; gRPC is the faithful AIP surface and the smaller lift.
- **Pure `cargo run --example` library snippets** — simplest, but a usage demo,
  not a server to test against.
- **A fresh minimal proto** — tighter, but duplicates proto setup and a second
  schema to maintain. Reusing freight keeps one source of truth.

## Consequences

- The demo can outrun the crates: handlers that call a not-yet-implemented
  primitive would hit its `todo!()`, so until an issue lands its handler uses a
  naive placeholder (counter IDs, full-replacement update) behind the `TODO(aip
  #N)` marker rather than calling the panicking API.
- The freight protos are now vendored in two places (here and `test-fixtures`).
  An edit to the shared einride sources must be mirrored; the duplication is the
  cost of each crate building standalone. A shared top-level `proto/` is the
  alternative if that drift becomes a burden.
