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

The Site store is backed by an in-memory SQLite database, so `ListSites`
filtering works out of the box (`aip-sql` transpiles the `filter` to
parameterized SQL and runs it there). Building the example therefore needs a C
toolchain for the bundled SQLite.

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
# UpdateShipper applies an AIP-134 update_mask via the typed `fieldmask::update`
# facade (#48): only `display_name` is masked, so it changes while the rest of the
# stored shipper is left untouched. Omit `display_name` from the request with the
# same mask and it is cleared instead.
gc -d '{"shipper":{"name":"shippers/$ID","display_name":"Acme Corp"},"updateMask":{"paths":["display_name"]}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
gc -d '{"name":"shippers/$ID"}'             127.0.0.1:50051 $SVC/DeleteShipper
```

Sites live under a shipper, and `ListSites` honors an AIP-132 `order_by`.
`CreateSite` also mints a system-assigned id, returning a name like
`shippers/$ID/sites/$SITE_ID`:

```sh
# Seed a couple of sites under the shipper, then list them ordered by name. Each
# carries `annotations` (a map) and `tags` (a list) for the has-operator filters.
gc -d '{"parent":"shippers/$ID","site":{"display_name":"Bravo","state":"STATE_ACTIVE","annotations":{"owner":"ops"},"tags":["refrigerated"]}}'  127.0.0.1:50051 $SVC/CreateSite
gc -d '{"parent":"shippers/$ID","site":{"display_name":"Alpha","state":"STATE_INACTIVE","annotations":{"region":"west"},"tags":["bulk"]}}'      127.0.0.1:50051 $SVC/CreateSite
gc -d '{"parent":"shippers/$ID","orderBy":"display_name"}'        127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","orderBy":"display_name desc"}'   127.0.0.1:50051 $SVC/ListSites

# Sorting and paging happen in SQL (#42): an ORDER BY plus a LIMIT/OFFSET derived
# from the page size and the offset page token. Ask for one site per page, then
# pass the returned `nextPageToken` back to get the next — the page boundaries
# stay stable because the resource name breaks ties. (Changing `orderBy` or
# `filter` mid-pagination flips the request checksum and rejects the token.)
gc -d '{"parent":"shippers/$ID","orderBy":"display_name","pageSize":1}'                       127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","orderBy":"display_name","pageSize":1,"pageToken":"<TOKEN>"}' 127.0.0.1:50051 $SVC/ListSites

# Bad syntax or an unknown ordering field is rejected with InvalidArgument —
# and the status carries AIP-193 details (#16): an ErrorInfo with a
# machine-readable reason (ORDER_BY_UNKNOWN_FIELD / domain aip-rs) plus a
# BadRequest naming the offending field. grpcurl prints the details block.
gc -d '{"parent":"shippers/$ID","orderBy":"bogus_field"}'         127.0.0.1:50051 $SVC/ListSites

# AIP-160 filtering (#39, #40, #41): the filter is type-checked, transpiled to
# parameterized SQL by `aip::sql`, and run in the in-memory SQLite store, so only
# matching sites come back. The full operator set lowers — `=` `!=` `<` `<=` `>`
# `>=`, `AND` `OR` `NOT` — over the scalar `display_name`/`name`, the nested
# numeric `lat_lng.latitude`, the timestamp `create_time`, and the enum `state`.
gc -d '{"parent":"shippers/$ID","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"state = STATE_ACTIVE"}'                127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"create_time > \"2024-01-01T00:00:00Z\""}' 127.0.0.1:50051 $SVC/ListSites

# The has operator `:` (#41): substring on a string, key presence in the
# `annotations` map, and membership in the `tags` list (a timestamp takes only
# `create_time:*` for presence). The map/list tests run through SQLite `json_each`.
gc -d '{"parent":"shippers/$ID","filter":"display_name:lph"}'     127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"annotations:owner"}'    127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"tags:refrigerated"}'    127.0.0.1:50051 $SVC/ListSites

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
| `UpdateShipper`   | `fieldmask` (typed `update` over `update_mask`) | #8, #48   | wired       |
| `DeleteShipper`   | `resourcename` (validate name)               | #4           | wired       |
| `CreateSite`      | `resourceid` (generate), `resourcename` (parse parent + format) | #5, #3 | wired       |
| `ListSites`       | `ordering` (parse/validate) + `pagination` (offset + checksum guard) + `filtering`/`aip-sql` (filter + `ORDER BY`/`LIMIT`/`OFFSET` → in-memory SQLite) | #9, #10, #6, #7, #11, #39, #40, #41, #42 | wired³ |
| `GetSite` / `UpdateSite` / `DeleteSite`, `BatchGetSites`, `*Shipment` | the same primitives + `filtering` | #11–#15 | `Unimplemented` |

³ `ListSites` parses the `order_by` (`aip::ordering::parse_order_by`) and
validates it against an allow-list of sortable Site paths (`validate_for_paths`,
#9). The descriptor-based `validate_for_message` (#10) guards that allow-list in
turn: a test checks every sortable path against the `Site` descriptor, so the
allow-list can't silently drift from the proto (and a sibling test checks every
path maps to a column in the schema). It then **sorts and pages in SQL** (#42):
`aip::sql::transpile_order_by` maps the validated `order_by` onto `ORDER BY`
columns through the same column schema as the filter, a resource-name tie-break is
appended for a total, stable order, and the SQLite store runs `ORDER BY …`
`LIMIT`/`OFFSET` derived from the page size and the offset page token — replacing
the old in-memory sort. Ordering composes with pagination: because `order_by` is a
non-pagination field, changing it mid-pagination flips the request checksum and
the now-stale page token is rejected — and `filter` is a non-pagination field too,
so it is covered by the same guard. `ListSites` also applies the AIP-160 `filter`:
`aip::filtering` parses and type-checks it, `aip::sql` transpiles it to a
parameterized `Predicate`, and the in-memory SQLite-backed Site store runs it
(#39, #40, #41). The transpiler lowers the full operator set the checker accepts —
`=` `!=` `<` `<=` `>` `>=`, `AND` `OR` `NOT`, and the has operator `:` —
recovering each operand's type from the declarations and a column schema
(ADR-0008): the scalar `display_name`/`name`, the timestamp `create_time` (bound
as RFC3339 text), the nested numeric `lat_lng.latitude`, the reflective enum
`state` (bound as its value name), and the `annotations` map / `tags` list. The
has operator `:` does substring on a string, key / element presence in the map /
list (via SQLite `json_each`), and presence on a timestamp (`create_time:*`).
Parent scoping is still an in-memory post-filter (`scope_to_parent` is #43). The
remaining Site/Shipment handlers await their methods (#11–#15) before they drop
`Unimplemented`.

² Real offset pagination through the `pagination` page-token codec (#6), with the
request-checksum guard (#7) that rejects a token when a non-pagination field
changes mid-pagination. `ListShippersRequest` carries only the pagination fields,
so its checksum is constant — `ListSites` exercises the guard against a varying
`parent`/`order_by`. Both list handlers open with one shared `parse_page(&req)?`
helper that folds the three-step preamble — checksum, token parse, page-size
resolution — and rejects a negative `page_size` with `INVALID_ARGUMENT` (#31). The
checksum is computed directly off the concrete request: the generated types are
Typed messages (#46), so `request_checksum` takes the request by its
`ReflectMessage` descriptor (#47) — no `DynamicMessage` bridge, no hand-derived
message name.

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
`prost_reflect::DescriptorPool` at runtime; that pool backs the `ReflectMessage`
derives below — each Typed message resolves its own descriptor from it.

The generated freight messages are **Typed messages** (#46): `build.rs`
adds `#[derive(prost_reflect::ReflectMessage)]` to each, enumerated from the same
`protox` set, so a message carries its own `MessageDescriptor`
(`Shipper::default().descriptor()`) without a by-name pool lookup — per
[ADR-0009](../../docs/adr/0009-reflective-typed-message-api.md). Every reflective
primitive the handlers call is now expressed over these Typed messages:
`ListShippers`/`ListSites` take `request_checksum` straight off the request's
descriptor (#47), and `UpdateShipper` applies its `update_mask` with the typed
`fieldmask::update` facade (#48) — so the server holds no `DynamicMessage` of its
own and the hand-rolled `reflect.rs` bridge is gone.

The freight protos and their vendored googleapis imports live under
[`proto/`](proto), so the example builds standalone. They are a copy of the same
einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto).
