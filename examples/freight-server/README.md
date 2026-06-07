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

Sites live under a shipper, and `ListSites` honors an AIP-132 `order_by`.
`CreateSite` also mints a system-assigned id, returning a name like
`shippers/$ID/sites/$SITE_ID`:

```sh
# Seed a couple of sites under the shipper, then list them ordered by name.
gc -d '{"parent":"shippers/$ID","site":{"display_name":"Bravo"}}' 127.0.0.1:50051 $SVC/CreateSite
gc -d '{"parent":"shippers/$ID","site":{"display_name":"Alpha"}}' 127.0.0.1:50051 $SVC/CreateSite
gc -d '{"parent":"shippers/$ID","orderBy":"display_name"}'        127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","orderBy":"display_name desc"}'   127.0.0.1:50051 $SVC/ListSites

# Bad syntax or an unknown ordering field is rejected with InvalidArgument —
# and the status carries AIP-193 details (#16): an ErrorInfo with a
# machine-readable reason (ORDER_BY_UNKNOWN_FIELD / domain aip-rs) plus a
# BadRequest naming the offending field. grpcurl prints the details block.
gc -d '{"parent":"shippers/$ID","orderBy":"bogus_field"}'         127.0.0.1:50051 $SVC/ListSites

# A missing required field is rejected the same way (here the server's own
# presence check, reason FIELD_REQUIRED / domain freight.example.com):
gc -d '{"shipper":{}}'                                            127.0.0.1:50051 $SVC/CreateShipper
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
| `CreateSite`      | `resourceid` (generate), `resourcename` (parse parent + format) | #5, #3 | wired       |
| `ListSites`       | `ordering` (parse/validate/sort) + `pagination` (offset + checksum guard) | #9, #10, #6, #7 | wired³ |
| `GetSite` / `UpdateSite` / `DeleteSite`, `BatchGetSites`, `*Shipment` | the same primitives + `filtering` | #11–#15 | `Unimplemented` |

³ `ListSites` parses the `order_by` (`aip::ordering::parse_order_by`) and
validates it against an allow-list of sortable Site paths (`validate_for_paths`,
#9) — the right gate here, because the in-memory `sort_sites` only knows those
curated paths. The descriptor-based `validate_for_message` (#10) guards that
allow-list in turn: a test checks every sortable path against the `Site`
descriptor, so the allow-list can't silently drift from the proto. `ListSites`
then applies the sort before paginating. Ordering composes with pagination:
because `order_by` is a non-pagination field, changing it mid-pagination flips
the request checksum and the now-stale page token is rejected. The remaining
Site/Shipment handlers await the `filtering` crate (#11–#15) and a `filter`
request field before they drop `Unimplemented`.

² Real offset pagination through the `pagination` page-token codec (#6), with the
request-checksum guard (#7) that rejects a token when a non-pagination field
changes mid-pagination. `ListShippersRequest` carries only the pagination fields,
so its checksum is constant — `ListSites` exercises the guard against a varying
`parent`/`order_by`. Both list handlers open with one shared `parse_page(&req)?`
helper that folds the three-step preamble — checksum, token parse, page-size
resolution — and rejects a negative `page_size` with `INVALID_ARGUMENT` (#31). The
checksum is computed reflectively, via a `DynamicMessage` built from the server's
descriptor pool; the request's message name is derived from its type
(`prost::Name`), not hand-typed.

## Errors (AIP-193)

Every handler returns rich [AIP-193](https://google.aip.dev/193) errors (#16).
A primitive's validation error converts straight to a `tonic::Status` with the
`?` operator: the crates' `From<Error> for tonic::Status` (behind their `tonic`
feature, enabled here) maps it to `INVALID_ARGUMENT` with an `ErrorInfo`
(machine-readable `reason` + `domain` `aip-rs`) and, where the error names a
field path, a `BadRequest` field violation — see
[`docs/adr/0007-aip193-error-details.md`](../../docs/adr/0007-aip193-error-details.md).
So `UpdateShipper`'s bad `update_mask` path, `ListSites`'s unknown `order_by`
field, and a stale page token all surface as structured errors with no per-call
wiring. The server's own presence and policy checks (e.g. a required
`display_name`, the shipper-name pattern) build the same details with
`tonic-types` directly, under the service's own domain.

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

`build.rs` also makes every generated message a **Typed message**
([ADR-0009](../../docs/adr/0009-reflective-typed-message-api.md), #46): it
enumerates the messages in the `protox`-built set and attaches
`#[derive(prost_reflect::ReflectMessage)]`, so a value carries its own
`Descriptor` — `Shipper::default().descriptor()` resolves it with no name-keyed
pool lookup. (`prost-reflect-build` automates this, but only by re-running
`protoc`; we replicate its attribute wiring against the protox set to keep the
no-`protoc` build.) The derive points at the same runtime `DESCRIPTOR_POOL`, so
this is the enabling step only — the `DynamicMessage` transcode bridge above is
unchanged here and is retired once the reflective primitives move to the typed
facade.

The freight protos and their vendored googleapis imports live under
[`proto/`](proto), so the example builds standalone. They are a copy of the same
einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto).
