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

With [`grpcurl`](https://github.com/fullstorydev/grpcurl) (the server speaks
gRPC server reflection, so no `-import-path`/`-proto` flags needed):

```sh
# List all served services via reflection.
grpcurl -plaintext 127.0.0.1:50051 list

SVC=einride.example.freight.v1.FreightService
gc() { grpcurl -plaintext "$@"; }

# CreateShipper mints a system-assigned id (a UUIDv4, per AIP-148), so it
# returns a name like `shippers/daf1cb3e-f33b-43f1-81cc-e65fda51efa5`. Copy that
# name from the response into the calls below (shown here as `shippers/$ID`).
gc -d '{"shipper":{"display_name":"Acme"}}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{}'                                  127.0.0.1:50051 $SVC/ListShippers
gc -d '{"name":"shippers/$ID"}'             127.0.0.1:50051 $SVC/GetShipper

# AIP-155 idempotency (#94): a `request_id` (a UUID) makes the create safe to
# retry. Send the SAME request twice with the same id — the second call replays
# the first response (same name) instead of minting a second shipper, so a
# network retry can't double-create. `aip::requestid` validates the id and names
# the replay/conflict contract; the demo keeps the seen ids in memory.
RID=$(uuidgen)
gc -d '{"shipper":{"display_name":"Acme"},"requestId":"'$RID'"}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{"shipper":{"display_name":"Acme"},"requestId":"'$RID'"}' 127.0.0.1:50051 $SVC/CreateShipper  # same name back
# Reusing that id with a DIFFERENT body is rejected with AlreadyExists +
# AIP-193 details (reason REQUEST_ID_CONFLICT / domain aip-rs).
gc -d '{"shipper":{"display_name":"Other"},"requestId":"'$RID'"}' 127.0.0.1:50051 $SVC/CreateShipper
# A malformed (non-UUID) id is InvalidArgument (reason REQUEST_ID_INVALID).
gc -d '{"shipper":{"display_name":"Acme"},"requestId":"not-a-uuid"}' 127.0.0.1:50051 $SVC/CreateShipper
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
**Resource name** and mutates it through the `aip::iam` structural helpers:

```sh
ic() { grpcurl -plaintext "$@"; }
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

```

`TestIamPermissions` (#68) decides the held subset of the requested permissions
*through* the opt-in cel-backed `eval` adapter (#66): it expands each **Binding**'s
**Role** to **Permissions** via an example-owned catalogue (`aip-iam` ships none —
ADR-0010), matches the caller (the same `x-freight-caller` header the read gate
reads), and evaluates any **Condition**. It returns only the held subset and never
errors on a permission the caller lacks. The catalogue maps `roles/freight.viewer`
(read verbs), `roles/freight.editor` (read + write), and `roles/freight.admin`
(+ IAM administration).

```sh
# Grant alice the freight viewer role on shippers/acme (first write may omit etag).
ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/freight.viewer","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# alice holds the read verb but not delete — only `freight.shippers.get` comes back.
ic -H 'x-freight-caller: user:alice@example.com' \
   -d '{"resource":"shippers/acme","permissions":["freight.shippers.get","freight.shippers.delete"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions
# bob is in no Binding, so he holds nothing — both are omitted (no error).
ic -H 'x-freight-caller: user:bob@example.com' \
   -d '{"resource":"shippers/acme","permissions":["freight.shippers.get","freight.shippers.delete"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions

# The subset changes with the policy: the editor role bundles the write verbs too,
# so delete now comes back as well.
ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/freight.editor","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
ic -H 'x-freight-caller: user:alice@example.com' \
   -d '{"resource":"shippers/acme","permissions":["freight.shippers.get","freight.shippers.delete"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions

# A conditional Binding is honoured through the eval adapter (version 3 required).
# A Condition that holds (request.time before the window) keeps the permission…
ic -d '{"resource":"shippers/acme","policy":{"version":3,"bindings":[{"role":"roles/freight.viewer","members":["user:alice@example.com"],"condition":{"expression":"request.time < timestamp(\"2030-01-01T00:00:00Z\")"}}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
ic -H 'x-freight-caller: user:alice@example.com' \
   -d '{"resource":"shippers/acme","permissions":["freight.shippers.get"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions
# …and a Condition that fails (the window has closed) excludes it — empty subset.
ic -d '{"resource":"shippers/acme","policy":{"version":3,"bindings":[{"role":"roles/freight.viewer","members":["user:alice@example.com"],"condition":{"expression":"request.time < timestamp(\"2020-01-01T00:00:00Z\")"}}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
ic -H 'x-freight-caller: user:alice@example.com' \
   -d '{"resource":"shippers/acme","permissions":["freight.shippers.get"]}' \
                                     127.0.0.1:50051 $IAMSVC/TestIamPermissions
```

Those Policies actually **govern freight access** (#67): the two services share one
policy store, so `GetShipper` consults the shipper's `Policy` and shapes an AIP-211
authorization error when the caller is not a granted **Member**. The demo reads the
caller identity from an `x-freight-caller` metadata header (a real server derives it
from authenticated transport); a resource with **no Policy is public**, mirroring the
open `ListShippers`, so the `GetShipper` above worked for anyone until you lock it
down. (`gc` and `ic` are the helper functions defined above.)

```sh
# Lock shippers/$ID down to alice (use the system-assigned name from CreateShipper).
ic -d '{"resource":"shippers/$ID","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# alice reads it; bob gets the canonical *non-leaking* PERMISSION_DENIED — message
# "Permission 'freight.shippers.get' denied on resource 'shippers/$ID' (or it might
# not exist)." with an IAM_PERMISSION_DENIED ErrorInfo (domain aip-rs).
gc -H 'x-freight-caller: user:alice@example.com' -d '{"name":"shippers/$ID"}' 127.0.0.1:50051 $SVC/GetShipper
gc -H 'x-freight-caller: user:bob@example.com'   -d '{"name":"shippers/$ID"}' 127.0.0.1:50051 $SVC/GetShipper

# Non-leaking: the *same* denial comes back for a name that does not exist, so an
# unauthorized caller cannot probe existence. Lock a never-created name and the
# parent collection (resource `shippers`) against bob, then GetShipper on the
# missing name is PERMISSION_DENIED too — indistinguishable from the existing one.
ic -d '{"resource":"shippers/ghost","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}' 127.0.0.1:50051 $IAMSVC/SetIamPolicy
ic -d '{"resource":"shippers","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}'       127.0.0.1:50051 $IAMSVC/SetIamPolicy
gc -H 'x-freight-caller: user:bob@example.com' -d '{"name":"shippers/ghost"}' 127.0.0.1:50051 $SVC/GetShipper

# AIP-211 fallback: a caller allowed to read the parent collection's children *is*
# told NOT_FOUND (it may know the resource is absent). Grant bob on the collection
# root, then the missing name comes back NOT_FOUND (reason IAM_RESOURCE_NOT_FOUND).
ic -d '{"resource":"shippers","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com","user:bob@example.com"]}]}}' 127.0.0.1:50051 $IAMSVC/SetIamPolicy
gc -H 'x-freight-caller: user:bob@example.com' -d '{"name":"shippers/ghost"}' 127.0.0.1:50051 $SVC/GetShipper
```

## What it exercises (and where it's headed)

The freight methods map onto the aip-rs primitives. Shipper is the worked
reference; Site and Shipment follow the same pattern as their handlers are
wired up.

| Method            | aip-rs primitive(s)                          | Issue        | Status      |
| ----------------- | -------------------------------------------- | ------------ | ----------- |
| `GetShipper`      | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `iam` (AIP-211 authorization → non-leaking `PERMISSION_DENIED` / `NOT_FOUND`-via-parent over the shared Policy store) | #4, #67, #82 | wired⁶ |
| `ListShippers`    | `pagination` (offset page-token codec + request-checksum guard) | #6, #7 | wired² |
| `CreateShipper`   | `resourceid` (generate), generated `ShipperResourceName` (format), `fieldbehavior` (clear OUTPUT_ONLY/IMMUTABLE, validate REQUIRED), `requestid` (AIP-155 `request_id` validation + idempotent replay) | #5, #3, #59, #82, #94 | wired       |
| `UpdateShipper`   | `fieldmask` (typed `update` over `update_mask`), `fieldbehavior` (copy OUTPUT_ONLY from existing, validate REQUIRED in mask) | #8, #48, #59 | wired       |
| `DeleteShipper`   | `resourcename` (validate) + generated `ShipperResourceName` (pattern match) | #4, #82      | wired       |
| `CreateSite`      | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `SiteResourceName` (format), `validation` (accumulate REQUIRED-field violations → AIP-193), `requestid` (AIP-155 idempotent replay) | #5, #3, #60, #82, #94 | wired       |
| `ListSites`       | `ordering` (parse/validate) + `pagination` (offset + checksum guard) + `filtering`/`aip-sql` (filter + server-composed scope/soft-delete + `ORDER BY`/`LIMIT`/`OFFSET` → in-memory SQLite), with the in-memory `filtering` matcher pinned against SQLite | #9, #10, #6, #7, #11, #39, #40, #41, #42, #43, #92 | wired³ |
| `CreateShipment`  | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `ShipmentResourceName` (format), `validation` (accumulate both REQUIRED endpoints → one AIP-193 response), `requestid` (AIP-155 idempotent replay) | #5, #3, #60, #82, #94 | wired⁴ |
| `ListShipments`   | `pagination` (offset + checksum guard) + `filtering`/`aip-sql` (filter + server-composed scope/soft-delete → in-memory SQLite) | #6, #7, #43 | wired⁴ |
| `IAMPolicy.GetIamPolicy` / `SetIamPolicy` | `iam` (Member validation + structural read-modify-write: dedupe/normalise, `etag` optimistic concurrency, conditions⟹version-3) over a decomposed SQLite policy store (iam-go's `iam_policy_bindings` schema) | #64, #65 | wired⁵ |
| `IAMPolicy.TestIamPermissions` | `iam` + the opt-in cel-backed `eval` adapter (`aip::iam::eval`): role→permission expansion via an example-owned catalogue, Member matching, Condition evaluation | #66, #68 | wired⁷ |
| `GetSite` / `UpdateSite` / `DeleteSite`, `BatchGetSites`, `GetShipment` / `UpdateShipment` / `DeleteShipment` | the same primitives | #11–#15 | `Unimplemented` |

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
could under-fill a page). The same checked `filter` is also evaluated **in memory**
by the reflective matcher (`aip::filtering::matches`, #92): an agreement test seeds a
Site corpus and asserts the matcher and the SQLite-backed `ListSites` select the same
sites for every advertised filter — so the in-memory and SQL paths can't drift on the
operator set, the enum/timestamp lowering, or the `NULL`/absent (three-valued) cases.
The remaining Site/Shipment handlers await their methods (#11–#15) before they drop
`Unimplemented`.

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
carrying its `etag`, or an empty one when none is set. Each row carries its
**Binding**'s **Condition** expression (the `condition` column, NULL when
unconditional) so a conditional grant round-trips and `TestIamPermissions` can
evaluate it (#68); `version` is not a stored column but reconstructed from the
*conditions ⟹ version 3* invariant (version 3 when any binding is conditional), and
a Condition's `title` / `description` are not persisted. The invariant itself is
still enforced *before* the write, so a conditional binding on an older version is
rejected up front. The example generates its own `IAMPolicy` *service* trait +
request types under `proto/`, but shares the `Policy` / `Binding` *message* layer
with `aip-iam` via `extern_path` (its opt-in `iam-proto` feature), so the service
operates on the very types the helpers mutate.

⁶ `GetShipper` enforces AIP-211 authorization (#67) using the `Policy` store it
**shares** with the `IAMPolicy` service: it reads the caller identity from an
`x-freight-caller` metadata header (a real server derives the principal from
authenticated transport), and a shipper with **no Policy attached is public** —
mirroring the open `ListShippers`, so existence is not secret until a Policy locks
it down. Once locked, an ungranted caller gets the canonical non-leaking
`PERMISSION_DENIED` from `aip::iam::authz::permission_denied` — *"Permission '{p}'
denied on resource '{r}' (or it might not exist)."* with an `IAM_PERMISSION_DENIED`
`ErrorInfo` — that hides whether the resource exists. A missing resource routes
through `aip::iam::authz::not_found_via_parent`: it returns the same
`PERMISSION_DENIED` unless the caller may read the parent collection's children, in
which case it is allowed to learn the resource is absent (`NOT_FOUND`). The gate is a
deliberately coarse **Member** membership check, not the role→permission expansion +
**Condition** evaluation that is the authorization *decision* — that lands behind the
opt-in cel-backed `eval` adapter (#66/#68, ADR-0010); #67 contributes the error
*shape*, not the decision.

⁷ `TestIamPermissions` (#68) is that authorization **decision**, made *through* the
opt-in cel-backed `eval` adapter (#66). Over the stored **Policy**, for each
**Binding** the caller is a **Member** of (matched the same way as the #67 read
gate), it evaluates any **Condition** — compiling the `google.type.Expr` (general
CEL) with `aip::iam::eval::Condition` and running it against a `RequestContext`
carrying `resource.name` (the resource under test) and `request.time` — and, when
the Condition holds, expands the **Binding**'s **Role** to its **Permissions**
through an **example-owned catalogue** (`role_permissions` in `src/iam.rs`):
`aip-iam` ships no role definitions, so the freight `roles/freight.viewer` /
`editor` / `admin` → permission mapping is the caller's (ADR-0010). It returns the
requested permissions intersected with that held set — a valid permission the caller
lacks is simply omitted, never an error — while a *malformed* requested permission
(not a `service.resource.verb`) and a *broken* stored **Condition** (invalid CEL, or
one that cannot evaluate to a bool) both surface as `INVALID_ARGUMENT` with an
`IAM_*` `ErrorInfo` via the AIP-193 mapping, the adapter keeping a broken Condition
distinct from one that simply did not hold. Unlike the read gate, an **unprotected**
resource (no Policy) holds **nothing** here — the held subset is decided purely from
the Policy's **Bindings**.

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

The Rust under [`src/gen`](src/gen) is emitted by `buf generate` and
**committed** ([ADR-0011](../../docs/adr/0011-buf-proto-pipeline.md)): there is
no codegen `build.rs`, so building the example needs no proto toolchain —
[`regen.sh`](regen.sh) is the maintainer path (buf + the BSR + a Rust
toolchain). The crate owns only its `einride.example.freight.v1` protos under
[`proto/`](proto); every shared `google.*` type is `extern_path`'d onto
[`aip-proto`](../../crates/aip-proto), googleapis itself riding as a
digest-pinned BSR dependency (`buf.lock`). That makes `google.iam.v1.Policy`
exist **once** by construction: the `IAMPolicy` service trait generated here
speaks the very `Policy` the `aip::iam` structural helpers operate on, where the
old `build.rs` hand-maintained eight per-message `extern_path` mappings to the
same end.

Each `google.api.resource` annotation also yields a **typed resource-name
wrapper** (`ShipperResourceName`, `SiteResourceName`, `ShipmentResourceName`)
via the workspace's own buf plugin,
[`protoc-gen-prost-aip`](../../crates/protoc-gen-prost-aip) (the
protoc-gen-go-aip analog, ADR-0011): the proto annotation is the single source
of truth for the name pattern, and the handlers parse and format resource names
through the generated `parse()`/`format()` instead of hand-written
`format!("shippers/{{shipper}}")` strings.

The generated freight messages are **Typed messages** (#46): the buf template
adds `#[derive(prost_reflect::ReflectMessage)]` to each, resolved against the
embedded descriptor pool (`buf build`'s import-complete image, which preserves
the extension bytes annotations ride on), so a message carries its own
`MessageDescriptor` (`Shipper::default().descriptor()`) without a by-name pool
lookup — per
[ADR-0009](../../docs/adr/0009-reflective-typed-message-api.md). Every reflective
primitive the handlers call is expressed over these Typed messages:
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

The freight proto sources under [`proto/`](proto) are a copy of the same
einride sources used by
[`crates/test-fixtures/proto`](../../crates/test-fixtures/proto); nothing
`google.*` is vendored anymore — those imports resolve from the BSR at the
commit pinned in [`buf.lock`](buf.lock). The `proto/imports.proto` anchor pulls
`google/iam/v1/iam_policy.proto` (the served `IAMPolicy` service, which no
freight proto imports) and `google/rpc/error_details.proto` (so the reflection
service exposes those descriptors and grpcurl can decode AIP-193 details) into the closure.
