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

## Amendment (ADR-0016): the Site and Shipment stores become typed-key tables

The typed-key store (ADR-0016) lands in freight, the proving ground for every
primitive. The Site/Shipment stores stay in-memory SQLite (the #39 engine choice
is unchanged); only the *row layout* and the *scoping/paging* mechanics change.

**Decision:** rework the `sites` and `shipments` schemas around the typed key
tuple.

- **Typed key columns, `name` dropped.** Each table's primary key is the
  resource's **Variables** as columns in **Pattern** order (`sites` keyed on
  `(shipper, site)`, `shipments` on `(shipper, shipment)`); the `name TEXT PRIMARY
  KEY` column is removed. The `name` field is reconstructed on read from the key
  columns through the generated wrapper's `Display` (ADR-0011 amendment),
  decomposed back into binds on write.
- **Column-per-field, no BLOB.** Each scalar field is its own typed column; small
  repeated/map fields (`tags`, `annotations`) are JSON columns. The ADR records
  normalized side tables as the production-correct layout for high cardinality.
- **Foreign key with cascade; `PRAGMA foreign_keys = ON`.** Each child table
  declares `FOREIGN KEY (parent key columns) REFERENCES parent (…) ON DELETE
  CASCADE`, and the SQLite pool turns foreign keys on per connection (a
  SQLite-only step; Postgres enforces them by default). The cascade is
  hard-delete only — soft delete stays a handler policy, so soft-deleting a
  **Shipper** leaves its **Sites**/**Shipments** visible-state untouched.
- **Scope by omission; cursor paging.** `ListSites`/`ListShipments` scope by
  composing `Predicate::eq` per concrete parent **Variable** and omitting per
  **Wildcard** (ADR-0008 amendment, fed by `Pattern::match_with_wildcards`,
  ADR-0002 amendment), and page with a **Cursor page token** over the key tuple
  (ADR-0004 amendment). `sites` gets one covering index, `(shipper, display_name,
  site)`.
- **`BatchGetSites` implemented.** It graduates from `Unimplemented` to `N` typed
  `get_site` primary-key lookups with the AIP-231 whole-batch-`NOT_FOUND` default.

**Consequences.** `cargo run -p freight-server` and every affected RPC keep
working; the README status table moves `BatchGetSites` to *wired* and the
`ListSites`/`ListShipments` rows note cursor paging, and the `grpcurl` flows are
refreshed. freight tests that hard-delete a **Shipper** without first clearing its
**Sites** now rely on the cascade rather than asserting orphans. As freight is the
sole consumer of the superseded scoping/paging surfaces, the old offset paging and
`scope_to_parent` usage are deleted here, not kept behind a flag.
