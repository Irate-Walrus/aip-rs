# freight-server

A runnable [tonic](https://github.com/hyperium/tonic) gRPC server that
demonstrates aip-rs end-to-end and gives the workspace something real to test
against. It implements einride's example `FreightService` (Shipper / Site /
Shipment) and the `google.iam.v1.IAMPolicy` service over an in-memory store.

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

# Server-side predicate composition (#43): the user `filter` above is never run
# alone. `aip::sql::Predicate` folds it together with the server's own predicates
# — an AIP parent scope (`name LIKE 'shippers/$ID/%'`, the parent escaped + bound)
# and a soft-delete (`delete_time IS NULL`) — into one fragment that owns
# precedence and placeholder numbering, so a user `a OR b` can't re-associate
# against the server's `AND`s. The scope runs in the SQL `WHERE`, so a site under
# another shipper never leaks into this listing.
gc -d '{"parent":"shippers/$ID","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites

# A missing required field is rejected the same way (here the server's own
# presence check, reason FIELD_REQUIRED / domain freight.example.com):
gc -d '{"shipper":{}}'                                            127.0.0.1:50051 $SVC/CreateShipper
```

Shipments live under a shipper too. `CreateShipment` mints a system-assigned id;
`ListShipments` runs the **same** server-side composition as `ListSites` (#43) —
parent scope + soft-delete + the user `filter` — against its own SQLite store, but
carries no `order_by`, so results come back in resource-name order:

```sh
# Seed a couple of shipments between sites, each carrying `annotations` (a map).
gc -d '{"parent":"shippers/$ID","shipment":{"origin_site":"shippers/$ID/sites/a","destination_site":"shippers/$ID/sites/b","annotations":{"priority":"high"}}}' 127.0.0.1:50051 $SVC/CreateShipment
gc -d '{"parent":"shippers/$ID","shipment":{"origin_site":"shippers/$ID/sites/b","destination_site":"shippers/$ID/sites/a","annotations":{"region":"west"}}}'   127.0.0.1:50051 $SVC/CreateShipment

# List all in-scope shipments, then filter: a `=` on a scalar column, and the has
# operator over the `annotations` map (via SQLite `json_each`). The parent scope
# and soft-delete are composed in automatically.
gc -d '{"parent":"shippers/$ID"}'                                                            127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"shippers/$ID","filter":"origin_site = \"shippers/$ID/sites/a\""}'          127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"shippers/$ID","filter":"annotations:priority"}'                            127.0.0.1:50051 $SVC/ListShipments
```

The `google.iam.v1.IAMPolicy` service (#64, #65) stores a **Policy** keyed by
**Resource name** and mutates it through the `aip::iam` structural helpers. Its
protos are vendored under this example's [`proto/`](proto) (not the shared
fixtures), so point grpcurl there:

```sh
IAM=examples/freight-server/proto
ic() { grpcurl -import-path "$IAM" -proto google/iam/v1/iam_policy.proto \
  -plaintext "$@"; }
IAMSVC=google.iam.v1.IAMPolicy

# GetIamPolicy on a resource with no policy returns an empty Policy (not an error).
ic -d '{"resource":"shippers/acme"}' 127.0.0.1:50051 $IAMSVC/GetIamPolicy

# SetIamPolicy validates every Member, enforces the conditions⟹version-3
# invariant, normalises the Policy (dedupe + canonical member/binding order),
# stamps a fresh content `etag`, and stores it through the aip::iam helpers (#65)
# — then echoes the stored Policy. A first write may omit the etag.
ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com","group:ops@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
# GetIamPolicy returns the stored Policy carrying that etag (base64 in JSON).
ic -d '{"resource":"shippers/acme"}' 127.0.0.1:50051 $IAMSVC/GetIamPolicy

# Read-modify-write: send the etag from the response above back in. A matching
# etag is accepted and the stored etag advances; replaying the now-stale one is
# rejected with ABORTED (the IAM optimistic-concurrency contract). Paste the
# base64 etag you got above:
ETAG='<paste the etag field from the response above>'
ic -d '{"resource":"shippers/acme","policy":{"version":1,"etag":"'"$ETAG"'","bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
# Replaying the same (now-stale) etag fails with ABORTED.
ic -d '{"resource":"shippers/acme","policy":{"version":1,"etag":"'"$ETAG"'","bindings":[{"role":"roles/editor","members":["user:bob@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# A malformed Member is rejected with InvalidArgument carrying an IAM_* ErrorInfo
# (reason IAM_MEMBER_UNKNOWN_TYPE / domain aip-rs) — the AIP-193 mapping (#16).
ic -d '{"resource":"shippers/acme","policy":{"bindings":[{"role":"roles/viewer","members":["robot:r2d2"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# A conditional Binding requires policy version 3 — version 1 is INVALID_ARGUMENT
# (reason IAM_POLICY_CONDITION_REQUIRES_VERSION_3).
ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"],"condition":{"expression":"request.time < timestamp(\"2030-01-01T00:00:00Z\")"}}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# TestIamPermissions is the next slice (#68): it decides through the opt-in
# cel-backed eval adapter (#66), so today it returns Unimplemented.
ic -d '{"resource":"shippers/acme","permissions":["freight.shippers.get"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions
```

## What it exercises (and where it's headed)

The freight methods map onto the aip-rs primitives. Shipper is the worked
reference; Site and Shipment follow the same pattern as their handlers are
wired up.

| Method            | aip-rs primitive(s)                          | Issue        | Status      |
| ----------------- | -------------------------------------------- | ------------ | ----------- |
| `GetShipper`      | `resourcename` (validate name)               | #4           | wired       |
| `ListShippers`    | `pagination` (offset page-token codec + request-checksum guard) | #6, #7 | wired² |
| `CreateShipper`   | `resourceid` (generate), `resourcename` (format), `fieldbehavior` (clear OUTPUT_ONLY/IMMUTABLE, validate REQUIRED) | #5, #3, #59 | wired       |
| `UpdateShipper`   | `fieldmask` (typed `update` over `update_mask`), `fieldbehavior` (copy OUTPUT_ONLY from existing, validate REQUIRED in mask) | #8, #48, #59 | wired       |
| `DeleteShipper`   | `resourcename` (validate name)               | #4           | wired       |
| `CreateSite`      | `resourceid` (generate), `resourcename` (parse parent + format), `validation` (accumulate REQUIRED-field violations → AIP-193) | #5, #3, #60 | wired       |
| `ListSites`       | `ordering` (parse/validate) + `pagination` (offset + checksum guard) + `filtering`/`aip-sql` (filter + server-composed scope/soft-delete + `ORDER BY`/`LIMIT`/`OFFSET` → in-memory SQLite) | #9, #10, #6, #7, #11, #39, #40, #41, #42, #43 | wired³ |
| `CreateShipment`  | `resourceid` (generate), `resourcename` (parse parent + format), `validation` (accumulate both REQUIRED endpoints → one AIP-193 response) | #5, #3, #60 | wired⁴ |
| `ListShipments`   | `pagination` (offset + checksum guard) + `filtering`/`aip-sql` (filter + server-composed scope/soft-delete → in-memory SQLite) | #6, #7, #43 | wired⁴ |
| `IAMPolicy.GetIamPolicy` / `SetIamPolicy` | `iam` (Member validation + structural read-modify-write: dedupe/normalise, `etag` optimistic concurrency, conditions⟹version-3) over a decomposed SQLite policy store (iam-go's `iam_policy_bindings` schema) | #64, #65 | wired⁵ |
| `GetSite` / `UpdateSite` / `DeleteSite`, `BatchGetSites`, `GetShipment` / `UpdateShipment` / `DeleteShipment` | the same primitives | #11–#15 | `Unimplemented` |
| `IAMPolicy.TestIamPermissions` | `iam` + the opt-in cel-backed `eval` adapter | #66, #68 | `Unimplemented` |

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
**Parent scoping now runs in the SQL `WHERE` too** (#43): rather than transpiling
the user filter alone, `ListSites` composes it through `aip::sql::Predicate` with
the server's own predicates — `Predicate::scope_to_parent("name", parent)` (a
`LIKE` prefix with the parent escaped + bound) and the soft-delete
`Predicate::is_null("delete_time")` — into one fragment that owns precedence and a
single coherent placeholder numbering. So a user `a OR b` is parenthesized under
the server's `AND`s instead of silently re-associating, and the page boundaries are
computed over exactly the in-scope, non-deleted rows (no in-memory post-filter that
could under-fill a page). The remaining Site/Shipment handlers await their methods
(#11–#15) before they drop `Unimplemented`.

⁴ `ListShipments` runs the **same** server-side composition as `ListSites` (#43)
against its own in-memory SQLite store: `aip::filtering` parses + type-checks the
AIP-160 `filter`, `aip::sql` transpiles it, and `scoped_predicate` folds it with
the parent scope and soft-delete through one `Predicate`. It carries no `order_by`,
so it orders by resource name for a total, stable page order, and the `filter` is a
non-pagination field, so the request-checksum guard (#7) covers it. `CreateShipment`
mirrors `CreateSite` — a system-assigned id (#5) formatted into the shipment
pattern (#3) — so there is something to list and filter; the other shipment
standard methods stay `Unimplemented` until their issues land.

⁵ The `google.iam.v1.IAMPolicy` service (#64, #65) over an in-memory SQLite
policy store. The **Policy** is stored *decomposed* into the `iam_policy_bindings`
table — one row per (resource, **Binding**, **Member**) — mirroring iam-go's
`iamspanner` schema, and reconstructed on read; its `etag` is a content digest
(`compute_etag`, a CRC32 over the canonical form) computed from that
reconstruction, not a stored column. `SetIamPolicy` runs the **Policy** through
the `aip::iam` helpers rather than blind-overwriting: it validates every
**Member** of every **Binding** via `aip::iam::Member` (a malformed member
converts through the crate's AIP-193 `From<Error>` (#16) to `INVALID_ARGUMENT`
with an `IAM_*` `ErrorInfo`), enforces the *conditions ⟹ version-3* invariant
(`aip::iam::policy::validate`), then checks the request `etag` against the stored
policy (`check_etag` — a stale token is `ABORTED`), normalises it
(dedupe + canonical order), and replaces the resource's rows in one transaction,
echoing the stored Policy back. `GetIamPolicy` reconstructs the stored Policy
carrying its `etag`, or an empty one when none is set. Like iam-go's `iamspanner`,
the schema persists neither `version` nor a **Binding**'s **Condition** — the
version-3 invariant is enforced *before* the write, so a conditional binding is
rejected up front while accepted policies round-trip as their `(role, members)`
grants. The example generates its own `IAMPolicy` *service* trait + request types
under `proto/`, but shares the `Policy` / `Binding` *message* layer with `aip-iam`
via `extern_path` (its opt-in `iam-proto` feature), so the service operates on the
very types the helpers mutate. `TestIamPermissions` is left as a seam — it decides
the held subset *through* the opt-in cel-backed `eval` adapter (#66), wired in #68
(ADR-0010).

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
wiring. The server's own presence and policy checks — the ones no aip-rs
primitive covers (e.g. a required `display_name`, both shipment endpoints, the
shipper-name pattern) — accumulate into an `aip::validation::Validator` (#60),
which resolves to the same AIP-193 details under the service's own domain. So a
`CreateShipment` missing both endpoints comes back with every violation in one
`BadRequest`, rather than one error per round-trip.

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

The same Typed messages and descriptor pool back `aip::reflect` (#61), which
reflects over the protos' **resource annotations** rather than their data: it
reads the `google.api.resource` / `google.api.resource_reference` options off the
descriptors. Because this is descriptor-time validation, not a request-path
primitive, it is proven by a test rather than wired into a handler — `proto.rs`'s
`freight_resource_references_resolve` runs `validate_resource_references` over the
generated `Shipment` / `BatchGetSitesRequest` and asserts a well-formed reference
(`origin_site` naming a `Site`) resolves while a mismatched one (a `Shipper` name
where a `Site` is required) is rejected.

The freight protos and their vendored googleapis imports live under
[`proto/`](proto), so the example builds standalone. The freight sources are a
copy of the same einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto); the
`google/iam/v1/*` set backing the `IAMPolicy` service is vendored from googleapis
(see [`proto/google/VENDOR.md`](proto/google/VENDOR.md)).
