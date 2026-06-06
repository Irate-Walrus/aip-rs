# freight-server

A runnable [tonic](https://github.com/hyperium/tonic) gRPC server that
demonstrates aip-rs end-to-end and gives the workspace something real to test
against. It implements einride's example `FreightService` (Shipper / Site /
Shipment) over an in-memory store.

It is a **living demo**: it compiles today, and each handler grows to use an
aip-rs crate as that crate's issue lands. Every seam is marked in
[`src/service.rs`](src/service.rs) with a `TODO(aip #N)` tied to its tracking
issue.

> **Implementing an issue?** Extending this server is part of the definition of
> done — see [`CLAUDE.md`](../../CLAUDE.md) at the repo root.

## Run

```sh
cargo run -p freight-server
# freight-server (aip-rs demo) listening on 127.0.0.1:50051
```

## Try it

With [`grpcurl`](https://github.com/fullstorydev/grpcurl) (no server reflection,
so point it at the shared proto tree):

```sh
PROTO=crates/test-fixtures/proto
SVC=einride.example.freight.v1.FreightService
gc() { grpcurl -import-path "$PROTO" \
  -proto einride/example/freight/v1/freight_service.proto \
  -plaintext "$@"; }

gc -d '{"shipper":{"display_name":"Acme"}}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{}'                                  127.0.0.1:50051 $SVC/ListShippers
gc -d '{"name":"shippers/1"}'               127.0.0.1:50051 $SVC/GetShipper
gc -d '{"shipper":{"name":"shippers/1","display_name":"Acme Corp"}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
gc -d '{"name":"shippers/1"}'               127.0.0.1:50051 $SVC/DeleteShipper
```

## What it exercises (and where it's headed)

The freight methods map onto the aip-rs primitives. Shipper is the worked
reference; Site and Shipment follow the same pattern as their handlers are
wired up.

| Method            | aip-rs primitive(s)                          | Issue        | Status      |
| ----------------- | -------------------------------------------- | ------------ | ----------- |
| `GetShipper`      | `resourcename` (validate name)               | #4           | wired¹      |
| `ListShippers`    | `pagination`                                 | #6, #7       | wired¹      |
| `CreateShipper`   | `resourceid` (generate), `resourcename` (format) | #5, #3   | #3 wired; #5 pending¹ |
| `UpdateShipper`   | `fieldmask` (apply `update_mask`)            | #8           | wired¹      |
| `DeleteShipper`   | `resourcename`                               | #4           | wired       |
| `*Site` / `*Shipment`, `BatchGetSites` | all of the above + `filtering`, `ordering` | #9–#15 | `Unimplemented` |

¹ Functional with naive placeholders today; the `TODO(aip #N)` seam swaps in the
real primitive when its issue lands.

## How the proto types are built

`build.rs` compiles the protos with [`protox`](https://crates.io/crates/protox) —
a pure-Rust protobuf compiler, so **no `protoc` is required** (matching
[ADR-0001](../../docs/adr/0001-prost-reflect-and-workspace.md)) — and feeds the
resulting `FileDescriptorSet` to `tonic-prost-build` for the message + service
codegen.

The freight protos and their vendored googleapis imports live under
[`proto/`](proto), so the example builds standalone. They are a copy of the same
einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto).
