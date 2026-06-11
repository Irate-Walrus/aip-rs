# Demo with a runnable freight gRPC server, grown issue by issue

> **Build mechanism superseded by [ADR-0011](0011-buf-proto-pipeline.md).** The
> demo originally compiled its own vendored protos with `protox` (no `protoc`)
> so it built standalone; ADR-0011's buf pipeline replaces that — a shared
> `aip-proto` crate with `google.*` from the BSR, generated code committed.
> The demo's purpose, gRPC surface, growth model, and storage decisions below
> stand.

aip-rs needs an executable end-to-end demo: a place to see the primitives used
together (resource names → IDs → pagination → field masks → filtering →
ordering → errors) and an integration-test surface the per-crate unit tests
can't provide. `test-fixtures` does **not** fill this role — it is a test-only
reflection harness with no generated service types, no storage, and nothing to
run.

We add `examples/freight-server`: a tonic gRPC server implementing einride's
example `FreightService` (Shipper / Site / Shipment), mirroring `aip-go`'s
`examples/examplelibrary`. It is a **living demo** — it compiles from day one
with the Shipper standard methods as the worked reference, and each handler
carries a `TODO(aip #N)` seam where a primitive plugs in as its issue lands;
the unimplemented methods return `Unimplemented`.

## Decisions

- **gRPC over the einride freight protos.** The freight methods map 1:1 onto
  the primitives (List → pagination/ordering/filtering, Update → field mask,
  names → resourcename/resourceid), so the demo exercises the whole SDK and
  reuses an existing schema rather than maintaining a second one.
- **In-memory storage; Sites and Shipments in in-memory SQLite** (amended by
  ADR-0008, #39). Shippers are keyed maps (ADR-0005) — the gRPC layer, not a
  datastore, is what exercises the primitives. The Site/Shipment stores are a
  real SQLite engine (`rusqlite`, bundled) opened in-memory, so an AIP-160
  **Filter** travels end-to-end into a database by default (`cargo run -p
  freight-server`, no feature flag) — at the cost of a C toolchain to build the
  example. The core crates remain datastore-free.
- **A workspace member, never published.** Lives under `examples/*` with
  `publish = false`; its server-only deps (`tokio`, `tonic-prost`,
  `tonic-prost-build`) stay out of the core crates.

## Considered Options

- **HTTP/REST via axum** using the `google.api.http` annotations — browser- and
  curl-friendly, but needs hand-written transcoding the protos define for gRPC.
  Rejected for v0.1; gRPC is the faithful AIP surface and the smaller lift.
- **Pure `cargo run --example` library snippets** — simplest, but a usage demo,
  not a server to test against.
- **A fresh minimal proto** — tighter, but duplicates proto setup and a second
  schema to maintain.

## Consequences

- The demo can outrun the crates: until an issue lands, its handler uses a
  naive placeholder (counter IDs, full-replacement update) behind the
  `TODO(aip #N)` marker rather than calling a not-yet-implemented, panicking
  API.
