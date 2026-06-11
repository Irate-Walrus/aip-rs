# freight-server

A runnable [tonic](https://github.com/hyperium/tonic) gRPC server that
demonstrates aip-rs end-to-end and gives the workspace something real to test
against. It implements einride's example `FreightService` (Shipper / Site /
Shipment) and the `google.iam.v1.IAMPolicy` service over an in-memory store.

> **Implementing an issue?** Extending this server is part of the definition of
> done — see [`CLAUDE.md`](../../CLAUDE.md) at the repo root.

## Run

```sh
cargo run -p freight-server
# freight-server (aip-rs demo) listening on 127.0.0.1:50051
```

The listen address defaults to `127.0.0.1:50051`. Override it with the
`FREIGHT_ADDR` environment variable:

```sh
FREIGHT_ADDR=0.0.0.0:8080 cargo run -p freight-server
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
# Every read carries an opaque AIP-154 `etag`. Copy it from the response
# (an 8-char hex string) into the read-modify-write calls below (as `$ETAG`).
gc -d '{"name":"shippers/$ID"}'             127.0.0.1:50051 $SVC/GetShipper

# AIP-155 idempotency: a `request_id` (a UUID) makes the create safe to
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
# AIP-163 validate_only: preview a create without committing. The full
# validation pipeline runs and the would-be shipper comes back (a system-assigned
# name + etag), but nothing is stored — ListShippers is unchanged afterwards, and
# a request that would fail (here a missing display_name) fails identically with
# the flag set.
gc -d '{"shipper":{"display_name":"Preview"},"validateOnly":true}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{"shipper":{},"validateOnly":true}'                         127.0.0.1:50051 $SVC/CreateShipper
# UpdateShipper applies an AIP-134 update_mask via the typed `fieldmask::update`
# facade: only `display_name` is masked, so it changes while the rest of the
# stored shipper is left untouched. Omit `display_name` from the request with the
# same mask and it is cleared instead. It also runs the AIP-154 read-modify-write:
# echo the etag you just read and the write succeeds, returning a *new* etag.
gc -d '{"shipper":{"name":"shippers/$ID","display_name":"Acme Corp","etag":"$ETAG"},"updateMask":{"paths":["display_name"]}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
# UpdateShipper honours validate_only too: preview the merged shipper —
# etag check, update mask, REQUIRED re-validation all run — without persisting it.
gc -d '{"shipper":{"name":"shippers/$ID","display_name":"Preview","etag":"$ETAG"},"updateMask":{"paths":["display_name"]},"validateOnly":true}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
# Replaying the now-stale etag is rejected with ABORTED (reason ETAG_MISMATCH) —
# the optimistic-concurrency guard against a racing writer. A garbage etag is
# InvalidArgument (reason ETAG_MALFORMED) instead; omitting it writes unconditionally.
gc -d '{"shipper":{"name":"shippers/$ID","display_name":"Clobber","etag":"$ETAG"},"updateMask":{"paths":["display_name"]}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
# DeleteShipper carries the etag on the request (it can't piggyback on the
# resource): the current etag permits the delete, a stale one is ABORTED. The
# delete is a *soft* delete (AIP-164): it stamps `delete_time` and keeps the
# record, returning the shipper rather than removing it.
gc -d '{"name":"shippers/$ID","etag":"$ETAG"}' 127.0.0.1:50051 $SVC/DeleteShipper
# A soft-deleted shipper is now hidden: a plain GetShipper is NOT_FOUND (reason
# SOFT_DELETE_NOT_FOUND), and ListShippers omits it — `aip::softdelete` owns the
# visibility rule and its AIP-193 mapping.
gc -d '{"name":"shippers/$ID"}'                127.0.0.1:50051 $SVC/GetShipper
gc -d '{}'                                     127.0.0.1:50051 $SVC/ListShippers
# Pass `showDeleted` to see it again — on the Get and in the List.
gc -d '{"name":"shippers/$ID","showDeleted":true}' 127.0.0.1:50051 $SVC/GetShipper
gc -d '{"showDeleted":true}'                       127.0.0.1:50051 $SVC/ListShippers
# UndeleteShipper clears the stamp; the shipper is live and visible again.
# Undeleting a shipper that is *not* deleted is ALREADY_EXISTS (reason
# SOFT_DELETE_NOT_DELETED) — there is nothing to recover.
gc -d '{"name":"shippers/$ID"}'                127.0.0.1:50051 $SVC/UndeleteShipper
gc -d '{"name":"shippers/$ID"}'                127.0.0.1:50051 $SVC/GetShipper
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

# Sorting and paging happen in SQL: an ORDER BY plus a LIMIT/OFFSET derived
# from the page size and the offset page token. Ask for one site per page, then
# pass the returned `nextPageToken` back to get the next — the page boundaries
# stay stable because the resource name breaks ties. (Changing `orderBy` or
# `filter` mid-pagination flips the request checksum and rejects the token.)
gc -d '{"parent":"shippers/$ID","orderBy":"display_name","pageSize":1}'                       127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","orderBy":"display_name","pageSize":1,"pageToken":"<TOKEN>"}' 127.0.0.1:50051 $SVC/ListSites

# Bad syntax or an unknown ordering field is rejected with InvalidArgument —
# and the status carries AIP-193 details: an ErrorInfo with a
# machine-readable reason (ORDER_BY_UNKNOWN_FIELD / domain aip-rs) plus a
# BadRequest naming the offending field. grpcurl prints the details block.
gc -d '{"parent":"shippers/$ID","orderBy":"bogus_field"}'         127.0.0.1:50051 $SVC/ListSites

# AIP-160 filtering: the filter is type-checked, transpiled to parameterized
# SQL by `aip::sql`, and run in the in-memory SQLite store, so only matching
# sites come back. The full operator set lowers — `=` `!=` `<` `<=` `>` `>=`,
# `AND` `OR` `NOT` — over the scalar `display_name`/`name`, the nested numeric
# `lat_lng.latitude`, the timestamp `create_time`, and the enum `state`.
gc -d '{"parent":"shippers/$ID","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"state = STATE_ACTIVE"}'                127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"create_time > \"2024-01-01T00:00:00Z\""}' 127.0.0.1:50051 $SVC/ListSites

# The has operator `:`: substring on a string, key presence in the `annotations`
# map, and membership in the `tags` list (a timestamp takes only `create_time:*`
# for presence). The map/list tests run through SQLite `json_each`.
gc -d '{"parent":"shippers/$ID","filter":"display_name:lph"}'     127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"annotations:owner"}'    127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"shippers/$ID","filter":"tags:refrigerated"}'    127.0.0.1:50051 $SVC/ListSites

# Server-side predicate composition: the user `filter` above is never run alone.
# `aip::sql::Predicate` folds it together with the server's own predicates — an
# AIP parent scope (`name LIKE 'shippers/$ID/%'`, the parent escaped + bound) and
# a soft-delete (`delete_time IS NULL`) — into one fragment that owns precedence
# and placeholder numbering, so a user `a OR b` can't re-associate against the
# server's `AND`s. The scope runs in the SQL `WHERE`, so a site under another
# shipper never leaks into this listing.
gc -d '{"parent":"shippers/$ID","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites

# A missing required field is rejected the same way (here the server's own
# presence check, reason FIELD_REQUIRED / domain freight.example.com):
gc -d '{"shipper":{}}'                                            127.0.0.1:50051 $SVC/CreateShipper
```

Shipments live under a shipper too. `CreateShipment` mints a system-assigned id;
`ListShipments` runs the **same** server-side composition as `ListSites` —
parent scope + soft-delete + the user `filter` — against its own SQLite store, but
carries no `order_by`, so results come back in resource-name order:

```sh
# Seed a couple of shipments between sites, each carrying `annotations` (a map).
# All six REQUIRED fields (AIP-203) must be set: both endpoints and the four
# pickup/delivery timestamps — a missing one is rejected (see below).
gc -d '{"parent":"shippers/$ID","shipment":{"origin_site":"shippers/$ID/sites/a","destination_site":"shippers/$ID/sites/b","pickup_earliest_time":"2024-01-01T08:00:00Z","pickup_latest_time":"2024-01-01T12:00:00Z","delivery_earliest_time":"2024-01-02T08:00:00Z","delivery_latest_time":"2024-01-02T12:00:00Z","annotations":{"priority":"high"}}}' 127.0.0.1:50051 $SVC/CreateShipment
gc -d '{"parent":"shippers/$ID","shipment":{"origin_site":"shippers/$ID/sites/b","destination_site":"shippers/$ID/sites/a","pickup_earliest_time":"2024-01-01T08:00:00Z","pickup_latest_time":"2024-01-01T12:00:00Z","delivery_earliest_time":"2024-01-02T08:00:00Z","delivery_latest_time":"2024-01-02T12:00:00Z","annotations":{"region":"west"}}}'   127.0.0.1:50051 $SVC/CreateShipment

# A bare shipment is rejected with every missing REQUIRED field in one AIP-193
# response (reason FIELD_REQUIRED / domain freight.example.com), accumulated by
# the reflective `aip-fieldbehavior` validator:
gc -d '{"parent":"shippers/$ID","shipment":{}}'                                              127.0.0.1:50051 $SVC/CreateShipment

# List all in-scope shipments, then filter: a `=` on a scalar column, and the has
# operator over the `annotations` map (via SQLite `json_each`). The parent scope
# and soft-delete are composed in automatically.
gc -d '{"parent":"shippers/$ID"}'                                                            127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"shippers/$ID","filter":"origin_site = \"shippers/$ID/sites/a\""}'          127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"shippers/$ID","filter":"annotations:priority"}'                            127.0.0.1:50051 $SVC/ListShipments
```

The `google.iam.v1.IAMPolicy` service stores a **Policy** keyed by
**Resource name** and mutates it through the `aip::iam` structural helpers:

```sh
ic() { grpcurl -plaintext "$@"; }
IAMSVC=google.iam.v1.IAMPolicy

# GetIamPolicy on a resource with no policy returns an empty Policy (not an error).
ic -d '{"resource":"shippers/acme"}' 127.0.0.1:50051 $IAMSVC/GetIamPolicy

# SetIamPolicy validates every Member, enforces the conditions⟹version-3
# invariant, normalises the Policy (dedupe + canonical member/binding order),
# stamps a fresh content `etag`, and stores it through the aip::iam helpers
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
# (reason IAM_MEMBER_UNKNOWN_TYPE / domain aip-rs) — the AIP-193 mapping.
ic -d '{"resource":"shippers/acme","policy":{"bindings":[{"role":"roles/viewer","members":["robot:r2d2"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# A conditional Binding requires policy version 3 — version 1 is INVALID_ARGUMENT
# (reason IAM_POLICY_CONDITION_REQUIRES_VERSION_3).
ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"],"condition":{"expression":"request.time < timestamp(\"2030-01-01T00:00:00Z\")"}}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

```

`TestIamPermissions` decides the held subset of the requested permissions
*through* the opt-in cel-backed `eval` adapter: it expands each **Binding**'s
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

Those Policies actually **govern freight access**: the two services share one
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

| Method            | aip-rs primitive(s)                          | Status      |
| ----------------- | -------------------------------------------- | ----------- |
| `GetShipper`      | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `iam` (AIP-211 authorization → non-leaking `PERMISSION_DENIED` / `NOT_FOUND`-via-parent over the shared Policy store), `softdelete` (AIP-164 `show_deleted` visibility gating) | wired |
| `ListShippers`    | `pagination` (offset page-token codec + request-checksum guard, read through the generated `PageRequest` impl) + `softdelete` (AIP-164 `show_deleted` filtering) | wired |
| `CreateShipper`   | `resourceid` (generate), generated `ShipperResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (clear OUTPUT_ONLY/IMMUTABLE, validate REQUIRED), `etag` (stamp the AIP-154 content etag), `requestid` (AIP-155 `request_id` validation + idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `UpdateShipper`   | `fieldmask` (typed `update` over `update_mask`), `fieldbehavior` (copy OUTPUT_ONLY from existing, validate REQUIRED in mask), `etag` (AIP-154 freshness check + re-stamp), `preview` (AIP-163 `validate_only` gate) | wired |
| `DeleteShipper`   | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `etag` (AIP-154 freshness check), `softdelete` (AIP-164 soft delete — stamp `delete_time`, keep the record) | wired |
| `UndeleteShipper` | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `softdelete` (AIP-164 undelete — clear `delete_time` after confirming the shipper is soft-deleted, else `ALREADY_EXISTS`) | wired |
| `CreateSite`      | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `SiteResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (reflective REQUIRED validation re-stamped to the service domain → AIP-193), `requestid` (AIP-155 idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `ListSites`       | `ordering` (parse/validate, read through the generated `OrderByRequest` impl) + `pagination` (offset + checksum guard, read through the generated `PageRequest` impl) + `filtering`/`aip-sql` (filter declarations derived from the `Site` descriptor + server-composed scope/soft-delete + `ORDER BY`/`LIMIT`/`OFFSET` → in-memory SQLite), with the in-memory `filtering` matcher pinned against SQLite | wired |
| `CreateShipment`  | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `ShipmentResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (reflective REQUIRED validation of all six fields — endpoints + four pickup/delivery timestamps — re-stamped to the service domain → one AIP-193 response), `requestid` (AIP-155 idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `ListShipments`   | `pagination` (offset + checksum guard, read through the generated `PageRequest` impl) + `filtering`/`aip-sql` (filter declarations derived from the `Shipment` descriptor + server-composed scope/soft-delete → in-memory SQLite) | wired |
| `IAMPolicy.GetIamPolicy` / `SetIamPolicy` | `iam` (Member validation + structural read-modify-write: dedupe/normalise, `etag` optimistic concurrency, conditions⟹version-3) over a decomposed SQLite policy store (iam-go's `iam_policy_bindings` schema) | wired |
| `IAMPolicy.TestIamPermissions` | `iam` + the opt-in cel-backed `eval` adapter (`aip::iam::eval`): role→permission expansion via an example-owned catalogue, Member matching, Condition evaluation | wired |
| `GetSite` / `UpdateSite` / `DeleteSite`, `BatchGetSites`, `GetShipment` / `UpdateShipment` / `DeleteShipment` | the same primitives | `Unimplemented` |
