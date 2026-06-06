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

# CreateShipper mints a system-assigned id (a UUIDv4, per AIP-148), so it
# returns a name like `shippers/daf1cb3e-f33b-43f1-81cc-e65fda51efa5`. Copy that
# name from the response into the calls below (shown here as `shippers/$ID`).
gc -d '{"shipper":{"display_name":"Acme"}}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{}'                                  127.0.0.1:50051 $SVC/ListShippers
gc -d '{"name":"shippers/$ID"}'             127.0.0.1:50051 $SVC/GetShipper
gc -d '{"shipper":{"name":"shippers/$ID","display_name":"Acme Corp"}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
gc -d '{"name":"shippers/$ID"}'             127.0.0.1:50051 $SVC/DeleteShipper
```

## What it exercises (and where it's headed)

The freight methods map onto the aip-rs primitives. Shipper is the worked
reference; Site and Shipment follow the same pattern as their handlers are
wired up.

| Method            | aip-rs primitive(s)                          | Issue        | Status      |
| ----------------- | -------------------------------------------- | ------------ | ----------- |
| `GetShipper`      | `resourcename` (validate name)               | #4           | wired       |
| `ListShippers`    | `pagination` (offset page-token codec + request-checksum guard) | #6, #7 | wired² |
| `CreateShipper`   | `resourceid` (generate), `resourcename` (format) | #5, #3   | wired       |
| `UpdateShipper`   | `fieldmask` (apply `update_mask`)            | #8           | wired       |
| `DeleteShipper`   | `resourcename` (validate name)               | #4           | wired       |
| `*Site` / `*Shipment`, `BatchGetSites` | all of the above + `ordering` (parse/validate) + `filtering` | #10–#15 | `Unimplemented` |

³ `ordering` parse and path-validation (#9) are library-ready; the handlers remain
`Unimplemented` pending the `filtering` crate (#11–#15) and `ordering`
`validate_for_message` (#10) needed to wire them fully.

² Real offset pagination through the `pagination` page-token codec (#6), with the
request-checksum guard (#7) that rejects a token when a non-pagination field
changes mid-pagination. `ListShippersRequest` carries only the pagination fields,
so its checksum is constant — `ListSites` (when wired) exercises the guard
against a varying `parent`/`skip`. The checksum is computed reflectively, via a
`DynamicMessage` built from the server's descriptor pool.

## How the proto types are built

`build.rs` compiles the protos with [`protox`](https://crates.io/crates/protox) —
a pure-Rust protobuf compiler, so **no `protoc` is required** (matching
[ADR-0001](../../docs/adr/0001-prost-reflect-and-workspace.md)) — and feeds the
resulting `FileDescriptorSet` to `tonic-prost-build` for the message + service
codegen. The same set is embedded raw so the server can build a
`prost_reflect::DescriptorPool` at runtime; that pool transcodes a generated
message to a `DynamicMessage` for the reflective primitives — `ListShippers`'
`request_checksum` (#7) and `UpdateShipper`'s `fieldmask` apply
([`src/reflect.rs`](src/reflect.rs)).

The freight protos and their vendored googleapis imports live under
[`proto/`](proto), so the example builds standalone. They are a copy of the same
einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto).
