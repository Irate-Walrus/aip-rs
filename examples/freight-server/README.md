# freight-server

A runnable [tonic](https://github.com/hyperium/tonic) gRPC server that
demonstrates aip-rs end-to-end and gives the workspace something real to test
against. It implements einride's example `FreightService` (Shipper / Site /
Shipment), the `google.iam.v1.IAMPolicy` service, and the
`google.longrunning.Operations` service over an in-memory store.

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

The stores are backed by an in-memory SQLite database — shippers, sites, and
shipments are typed-key tables in one connection — so `ListSites` filtering works
out of the box (`aip-sql` transpiles the `filter` to parameterized SQL and runs it
there) and a hard-deleted shipper cascades to its children. Building the example
therefore needs a C toolchain for the bundled SQLite.

## Try it

With [`grpcurl`](https://github.com/fullstorydev/grpcurl) and
[`jq`](https://jqlang.org) (the server speaks gRPC server reflection, so no
`-import-path`/`-proto` flags needed):

```sh
# List all served services via reflection.
grpcurl -plaintext 127.0.0.1:50051 list

SVC=einride.example.freight.v1.FreightService
gc() { grpcurl -plaintext "$@"; }

# CreateShipper mints a system-assigned id (a UUIDv4, per AIP-148). Capture the
# returned name — a `shippers/<uuid>` string — into $ID for the calls below.
ID=$(gc -d '{"shipper":{"display_name":"Acme"}}' 127.0.0.1:50051 $SVC/CreateShipper | jq -r .name)
gc -d '{}'                                  127.0.0.1:50051 $SVC/ListShippers
# Every read carries an opaque AIP-154 `etag` (an 8-char hex string). Capture it
# from GetShipper into $ETAG for the read-modify-write calls below.
ETAG=$(gc -d '{"name":"'"$ID"'"}' 127.0.0.1:50051 $SVC/GetShipper | jq -r .etag)

# AIP-155 idempotency: a `request_id` (a UUID) makes the create safe to
# retry. Send the SAME request twice with the same id — the second call replays
# the first response (same name) instead of minting a second shipper, so a
# network retry can't double-create. `aip::requestid` validates the id and names
# the replay/conflict contract; the demo keeps the seen ids in memory.
RID=$(uuidgen)
gc -d '{"shipper":{"display_name":"Acme"},"request_id":"'"$RID"'"}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{"shipper":{"display_name":"Acme"},"request_id":"'"$RID"'"}' 127.0.0.1:50051 $SVC/CreateShipper  # same name back
# Reusing that id with a DIFFERENT body is rejected with AlreadyExists +
# AIP-193 details (reason REQUEST_ID_CONFLICT / domain freight.example.com —
# the one service domain every error carries; see "Error domain" below).
gc -d '{"shipper":{"display_name":"Other"},"request_id":"'"$RID"'"}' 127.0.0.1:50051 $SVC/CreateShipper
# A malformed (non-UUID) id is InvalidArgument (reason REQUEST_ID_INVALID).
gc -d '{"shipper":{"display_name":"Acme"},"request_id":"not-a-uuid"}' 127.0.0.1:50051 $SVC/CreateShipper
# AIP-163 validate_only: preview a create without committing. The full
# validation pipeline runs and the would-be shipper comes back (a system-assigned
# name + etag), but nothing is stored — ListShippers is unchanged afterwards, and
# a request that would fail (here a missing display_name) fails identically with
# the flag set.
gc -d '{"shipper":{"display_name":"Preview"},"validate_only":true}' 127.0.0.1:50051 $SVC/CreateShipper
gc -d '{"shipper":{},"validate_only":true}'                         127.0.0.1:50051 $SVC/CreateShipper
# UpdateShipper applies an AIP-134 update_mask via the typed `fieldmask::update`
# facade: only `display_name` is masked, so it changes while the rest of the
# stored shipper is left untouched. Omit `display_name` from the request with the
# same mask and it is cleared instead. It also runs the AIP-154 read-modify-write:
# the current etag permits the write and returns a *new* etag — capture that.
OLD_ETAG=$ETAG
ETAG=$(gc -d '{"shipper":{"name":"'"$ID"'","display_name":"Acme Corp","etag":"'"$OLD_ETAG"'"},"update_mask":{"paths":["display_name"]}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper | jq -r .etag)
# UpdateShipper honours validate_only too: preview the merged shipper —
# etag check, update mask, REQUIRED re-validation all run — without persisting it.
gc -d '{"shipper":{"name":"'"$ID"'","display_name":"Preview","etag":"'"$ETAG"'"},"update_mask":{"paths":["display_name"]},"validate_only":true}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
# Replaying the now-stale etag is rejected with ABORTED (reason ETAG_MISMATCH) —
# the optimistic-concurrency guard against a racing writer. A garbage etag is
# InvalidArgument (reason ETAG_MALFORMED) instead; omitting it writes unconditionally.
gc -d '{"shipper":{"name":"'"$ID"'","display_name":"Clobber","etag":"'"$OLD_ETAG"'"},"update_mask":{"paths":["display_name"]}}' \
                                            127.0.0.1:50051 $SVC/UpdateShipper
# DeleteShipper carries the etag on the request (it can't piggyback on the
# resource): the current etag permits the delete, a stale one is ABORTED. The
# delete is a *soft* delete (AIP-164): it stamps `delete_time` and keeps the
# record, returning the shipper rather than removing it.
ETAG=$(gc -d '{"name":"'"$ID"'","etag":"'"$ETAG"'"}' 127.0.0.1:50051 $SVC/DeleteShipper | jq -r .etag)
# A soft-deleted shipper is now hidden: a plain GetShipper is NOT_FOUND (reason
# SOFT_DELETE_NOT_FOUND), and ListShippers omits it — `aip::softdelete` owns the
# visibility rule and its AIP-193 mapping.
gc -d '{"name":"'"$ID"'"}'                127.0.0.1:50051 $SVC/GetShipper
gc -d '{}'                                127.0.0.1:50051 $SVC/ListShippers
# Pass `show_deleted` to see it again — on the Get and in the List.
gc -d '{"name":"'"$ID"'","show_deleted":true}' 127.0.0.1:50051 $SVC/GetShipper
gc -d '{"show_deleted":true}'                  127.0.0.1:50051 $SVC/ListShippers
# UndeleteShipper clears the stamp; the shipper is live and visible again.
# Undeleting a shipper that is *not* deleted is ALREADY_EXISTS (reason
# SOFT_DELETE_NOT_DELETED) — there is nothing to recover.
gc -d '{"name":"'"$ID"'"}' 127.0.0.1:50051 $SVC/UndeleteShipper
gc -d '{"name":"'"$ID"'"}' 127.0.0.1:50051 $SVC/GetShipper
```

Sites live under a shipper, and `ListSites` honors an AIP-132 `order_by`.
`CreateSite` also mints a system-assigned id, returning a name like
`shippers/$ID/sites/$SITE_ID`. Capture both site names — they are used in the
Shipment section below:

```sh
# Seed a couple of sites under the shipper, then list them ordered by name. Each
# carries `annotations` (a map) and `tags` (a list) for the has-operator filters.
SITE_BRAVO=$(gc -d '{"parent":"'"$ID"'","site":{"display_name":"Bravo","state":"STATE_ACTIVE","annotations":{"owner":"ops"},"tags":["refrigerated"]}}' \
                                           127.0.0.1:50051 $SVC/CreateSite | jq -r .name)
SITE_ALPHA=$(gc -d '{"parent":"'"$ID"'","site":{"display_name":"Alpha","state":"STATE_INACTIVE","annotations":{"region":"west"},"tags":["bulk"]}}' \
                                           127.0.0.1:50051 $SVC/CreateSite | jq -r .name)
gc -d '{"parent":"'"$ID"'","order_by":"display_name"}'      127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"'"$ID"'","order_by":"display_name desc"}' 127.0.0.1:50051 $SVC/ListSites

# Sorting and paging happen in SQL: an ORDER BY plus a cursor seek over the key
# tuple — the page token carries the last row's key, not an offset. Ask for one
# site per page, then pass the returned `nextPageToken` back to get the next — the
# page boundaries stay stable because the key columns break ties. (Changing
# `order_by` or `filter` mid-pagination flips the request checksum and rejects the
# token; an old offset-format token is rejected outright so the client restarts.)
TOKEN=$(gc -d '{"parent":"'"$ID"'","order_by":"display_name","page_size":1}' \
                                           127.0.0.1:50051 $SVC/ListSites | jq -r '.nextPageToken // empty')
gc -d '{"parent":"'"$ID"'","order_by":"display_name","page_size":1,"page_token":"'"$TOKEN"'"}' \
                                           127.0.0.1:50051 $SVC/ListSites

# The cursor seek is direction-aware, so a descending order pages correctly past
# the first page too — page two resumes *below* the last row, not above it.
DTOKEN=$(gc -d '{"parent":"'"$ID"'","order_by":"display_name desc","page_size":1}' \
                                           127.0.0.1:50051 $SVC/ListSites | jq -r '.nextPageToken // empty')
gc -d '{"parent":"'"$ID"'","order_by":"display_name desc","page_size":1,"page_token":"'"$DTOKEN"'"}' \
                                           127.0.0.1:50051 $SVC/ListSites

# Bad syntax or an unknown ordering field is rejected with InvalidArgument —
# and the status carries AIP-193 details: an ErrorInfo with a
# machine-readable reason (ORDER_BY_UNKNOWN_FIELD / domain freight.example.com)
# plus a BadRequest naming the offending field. grpcurl prints the details block.
gc -d '{"parent":"'"$ID"'","order_by":"bogus_field"}' 127.0.0.1:50051 $SVC/ListSites

# AIP-160 filtering: the filter is type-checked, transpiled to parameterized
# SQL by `aip::sql`, and run in the in-memory SQLite store, so only matching
# sites come back. The full operator set lowers — `=` `!=` `<` `<=` `>` `>=`,
# `AND` `OR` `NOT` — over the scalar `display_name`, the nested numeric
# `lat_lng.latitude`, the timestamp `create_time`, and the enum `state`.
gc -d '{"parent":"'"$ID"'","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"'"$ID"'","filter":"state = STATE_ACTIVE"}'                127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"'"$ID"'","filter":"create_time > \"2024-01-01T00:00:00Z\""}' 127.0.0.1:50051 $SVC/ListSites

# The has operator `:`: substring on a string, key presence in the `annotations`
# map, and membership in the `tags` list (a timestamp takes only `create_time:*`
# for presence). The map/list tests run through SQLite `json_each`.
gc -d '{"parent":"'"$ID"'","filter":"display_name:lph"}'     127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"'"$ID"'","filter":"annotations:owner"}'    127.0.0.1:50051 $SVC/ListSites
gc -d '{"parent":"'"$ID"'","filter":"tags:refrigerated"}'    127.0.0.1:50051 $SVC/ListSites

# Server-side predicate composition: the user `filter` above is never run alone.
# `aip::sql::Predicate` folds it together with the server's own predicates — an
# AIP parent scope (one equality per concrete parent key column, e.g. `shipper =
# '<id>'`) and a soft-delete (`delete_time IS NULL`) — into one fragment that owns
# precedence and placeholder numbering, so a user `a OR b` can't re-associate
# against the server's `AND`s. The scope runs in the SQL `WHERE`, so a site under
# another shipper never leaks into this listing.
gc -d '{"parent":"'"$ID"'","filter":"display_name = \"Alpha\" OR display_name = \"Bravo\""}' 127.0.0.1:50051 $SVC/ListSites

# A `-` wildcard in the parent lists sites across every shipper: the equality is
# omitted rather than over-matching across name segments.
gc -d '{"parent":"shippers/-"}' 127.0.0.1:50051 $SVC/ListSites

# BatchGetSites fetches several sites by name in one call (AIP-231): each name is a
# primary-key lookup, returned in request order. Any missing name fails the whole
# batch with NOT_FOUND rather than returning a partial result.
gc -d '{"parent":"'"$ID"'","names":["'"$SITE_BRAVO"'","'"$SITE_ALPHA"'"]}' 127.0.0.1:50051 $SVC/BatchGetSites

# A missing required field is rejected the same way (the reflective
# `aip-fieldbehavior` validator; the BadRequest names `shipper.display_name`,
# reason FIELD_REQUIRED / domain freight.example.com):
gc -d '{"shipper":{}}'                                       127.0.0.1:50051 $SVC/CreateShipper
```

Shipments live under a shipper too. `CreateShipment` mints a system-assigned id;
`ListShipments` runs the **same** server-side composition as `ListSites` —
parent scope + soft-delete + the user `filter` — against its own SQLite store, but
carries no `order_by`, so results come back in key (`shipper`, `shipment`) order:

```sh
# Seed a couple of shipments between the sites created above, each carrying
# `annotations` (a map). All six REQUIRED fields (AIP-203) must be set: both
# endpoints and the four pickup/delivery timestamps — a missing one is rejected
# (see below).
gc -d '{"parent":"'"$ID"'","shipment":{"origin_site":"'"$SITE_BRAVO"'","destination_site":"'"$SITE_ALPHA"'","pickup_earliest_time":"2024-01-01T08:00:00Z","pickup_latest_time":"2024-01-01T12:00:00Z","delivery_earliest_time":"2024-01-02T08:00:00Z","delivery_latest_time":"2024-01-02T12:00:00Z","annotations":{"priority":"high"}}}' 127.0.0.1:50051 $SVC/CreateShipment
gc -d '{"parent":"'"$ID"'","shipment":{"origin_site":"'"$SITE_ALPHA"'","destination_site":"'"$SITE_BRAVO"'","pickup_earliest_time":"2024-01-01T08:00:00Z","pickup_latest_time":"2024-01-01T12:00:00Z","delivery_earliest_time":"2024-01-02T08:00:00Z","delivery_latest_time":"2024-01-02T12:00:00Z","annotations":{"region":"west"}}}' 127.0.0.1:50051 $SVC/CreateShipment

# A bare shipment is rejected with every missing REQUIRED field in one AIP-193
# response (reason FIELD_REQUIRED / domain freight.example.com), accumulated by
# the reflective `aip-fieldbehavior` validator:
gc -d '{"parent":"'"$ID"'","shipment":{}}'                                              127.0.0.1:50051 $SVC/CreateShipment

# List all in-scope shipments, then filter: a `=` on a scalar column, and the has
# operator over the `annotations` map (via SQLite `json_each`). The parent scope
# and soft-delete are composed in automatically.
gc -d '{"parent":"'"$ID"'"}'                                                            127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"'"$ID"'","filter":"origin_site = \"'"$SITE_BRAVO"'\""}'               127.0.0.1:50051 $SVC/ListShipments
gc -d '{"parent":"'"$ID"'","filter":"annotations:priority"}'                            127.0.0.1:50051 $SVC/ListShipments
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
# — then echoes the stored Policy. A first write may omit the etag. Capture
# the returned etag (base64 in JSON) for the read-modify-write call below.
IAM_ETAG=$(ic -d '{"resource":"shippers/acme","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com","group:ops@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy | jq -r .etag)
# GetIamPolicy returns the stored Policy carrying that etag.
ic -d '{"resource":"shippers/acme"}' 127.0.0.1:50051 $IAMSVC/GetIamPolicy

# Read-modify-write: send the etag back in. A matching etag is accepted and the
# stored etag advances; replaying the now-stale one is rejected with ABORTED
# (the IAM optimistic-concurrency contract).
ic -d '{"resource":"shippers/acme","policy":{"version":1,"etag":"'"$IAM_ETAG"'","bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy
# Replaying the same (now-stale) etag fails with ABORTED.
ic -d '{"resource":"shippers/acme","policy":{"version":1,"etag":"'"$IAM_ETAG"'","bindings":[{"role":"roles/editor","members":["user:bob@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# A malformed Member is rejected with InvalidArgument carrying an IAM_* ErrorInfo
# (reason IAM_MEMBER_UNKNOWN_TYPE / domain freight.example.com) — the AIP-193 mapping.
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
# Lock $ID down to alice.
ic -d '{"resource":"'"$ID"'","policy":{"version":1,"bindings":[{"role":"roles/viewer","members":["user:alice@example.com"]}]}}' \
                                     127.0.0.1:50051 $IAMSVC/SetIamPolicy

# alice reads it; bob gets the canonical *non-leaking* PERMISSION_DENIED — message
# "Permission 'freight.shippers.get' denied on resource '$ID' (or it might not
# exist)." with an IAM_PERMISSION_DENIED ErrorInfo (domain freight.example.com).
gc -H 'x-freight-caller: user:alice@example.com' -d '{"name":"'"$ID"'"}' 127.0.0.1:50051 $SVC/GetShipper
gc -H 'x-freight-caller: user:bob@example.com'   -d '{"name":"'"$ID"'"}' 127.0.0.1:50051 $SVC/GetShipper

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

### Long-running operations (AIP-151)

`BatchCreateShippers` is slow, so it returns a `google.longrunning.Operation`
instead of blocking. The client polls `GetOperation` on the
`google.longrunning.Operations` service until the operation is `done`; the
operation then carries a `BatchCreateShippersResponse`, and its `metadata` shows
progress on every poll. `aip-rs` owns the `Operation` state machine and its
errors; the work, the store, and the service are the server's (ADR-0015).

```sh
gc() { grpcurl -plaintext "$@"; }
SVC=einride.example.freight.v1.FreightService
OPS=google.longrunning.Operations

# Start the batch. It returns an Operation right away with `done` unset and a
# server-minted `name` like `operations/3f2a…`. Capture the name to poll on.
OP=$(gc -d '{"requests":[{"shipper":{"display_name":"Acme"}},{"shipper":{"display_name":"Globex"}}]}' \
  127.0.0.1:50051 $SVC/BatchCreateShippers | grep -o '"name": *"[^"]*"' | head -1 | cut -d'"' -f4)
echo "operation: $OP"

# Poll. While it runs, `metadata` reports `created`/`total`; once `done` is true
# the `response` carries the created shippers (each with a minted name + etag).
gc -d '{"name":"'"$OP"'"}' 127.0.0.1:50051 $OPS/GetOperation
# ... repeat until the response shows `"done": true` ...

# WaitOperation blocks server-side until the operation finishes (or the capped
# timeout elapses), so a client need not spin on GetOperation.
gc -d '{"name":"'"$OP"'","timeout":"30s"}' 127.0.0.1:50051 $OPS/WaitOperation

# Cancel flow: start another batch and cancel it mid-flight. Cancellation is
# best-effort — CancelOperation returns immediately, and the operation soon polls
# back `done` with an `error` of CANCELLED (code 1).
OP2=$(gc -d '{"requests":[{"shipper":{"display_name":"A"}},{"shipper":{"display_name":"B"}},{"shipper":{"display_name":"C"}},{"shipper":{"display_name":"D"}},{"shipper":{"display_name":"E"}}]}' \
  127.0.0.1:50051 $SVC/BatchCreateShippers | grep -o '"name": *"[^"]*"' | head -1 | cut -d'"' -f4)
gc -d '{"name":"'"$OP2"'"}' 127.0.0.1:50051 $OPS/CancelOperation
gc -d '{"name":"'"$OP2"'"}' 127.0.0.1:50051 $OPS/GetOperation

# validate_only previews the would-be shippers in an already-`done` operation
# with an empty `name` (no state kept) and persists nothing (AIP-163).
gc -d '{"requests":[{"shipper":{"display_name":"Acme"}}],"validate_only":true}' \
  127.0.0.1:50051 $SVC/BatchCreateShippers
```

## Error domain

Every error a client sees carries one AIP-193 `ErrorInfo.domain`:
`freight.example.com`. That is the contract AIP-193 wants — a service presents a
single domain across its whole error surface — and the demo gets it for free,
set once, not re-stamped at every call site (aip #145, ADR-0007).

The aip-rs primitives can't know the deploying service, so each maps its errors
under the library sentinel domain `aip-rs`, meaning "replace me at the boundary".
`main.rs` installs the `aip::errordomain` layer on the server builder:

```rust
Server::builder()
    .layer(aip::errordomain::Layer::new(SERVICE_DOMAIN)) // SERVICE_DOMAIN = "freight.example.com"
    .add_service(FreightServiceServer::new(server))
```

On the way out, the layer rewrites the `aip-rs` sentinel in
`grpc-status-details-bin` to `freight.example.com` — in both the headers
(trailers-only unary errors) and the trailers (streaming errors). A domain the
service raised itself (the `Validator` checks behind the shipper-name pattern,
which already use `freight.example.com`) is left untouched, since it is not the
sentinel. So the handlers convert library errors with a bare `?` and never carry
the domain themselves.

## What it exercises (and where it's headed)

The freight methods map onto the aip-rs primitives. Shipper is the worked
reference; Site and Shipment follow the same pattern as their handlers are
wired up.

| Method            | aip-rs primitive(s)                          | Status      |
| ----------------- | -------------------------------------------- | ----------- |
| `GetShipper`      | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `iam` (AIP-211 authorization → non-leaking `PERMISSION_DENIED` / `NOT_FOUND`-via-parent over the shared Policy store), `softdelete` (AIP-164 `show_deleted` visibility gating) | wired |
| `ListShippers`    | `pagination` (`Page::parse` folds the request-checksum guard + cursor token decode + AIP-158 size policy via `SizeLimits`, then `fetch_limit`/`split_overfetch` drive a keyset seek over the `shippers` table and mint the follow-on token; read through the generated `PageRequest` impl) + `softdelete` (AIP-164 `show_deleted` filtering, run as `delete_time IS NULL` in SQL) | wired |
| `CreateShipper`   | `resourceid` (generate), generated `ShipperResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (clear OUTPUT_ONLY/IMMUTABLE, validate REQUIRED), `etag` (stamp the AIP-154 content etag), `requestid` (AIP-155 `request_id` validation + idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `UpdateShipper`   | `fieldmask` (typed `update` over `update_mask`), `fieldbehavior` (copy OUTPUT_ONLY from existing, validate REQUIRED in mask), `etag` (AIP-154 freshness check + re-stamp), `preview` (AIP-163 `validate_only` gate) | wired |
| `DeleteShipper`   | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `etag` (AIP-154 freshness check), `softdelete` (AIP-164 soft delete — stamp `delete_time`, keep the record) | wired |
| `UndeleteShipper` | `resourcename` (validate) + generated `ShipperResourceName` (pattern match), `softdelete` (AIP-164 undelete — clear `delete_time` after confirming the shipper is soft-deleted, else `ALREADY_EXISTS`) | wired |
| `CreateSite`      | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `SiteResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (reflective REQUIRED validation → AIP-193, the `aip-rs` sentinel rewritten to the service domain by the boundary layer), `requestid` (AIP-155 idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `ListSites`       | `ordering` (parse/validate, read through the generated `OrderByRequest` impl) + `pagination` (`Page::parse` preamble: checksum guard + cursor token + `SizeLimits` size policy, then `fetch_limit`/`split_overfetch` drive the store overfetch probe whose seek is a direction-aware `Predicate::keyset_seek` over the order_by columns plus the key tie-break (the efficient row-value form when all ascending, the lexicographic disjunction when a column is descending), and mint the follow-on token; read through the generated `PageRequest` impl) + `filtering`/`aip-sql` (filter declarations derived from the `Site` descriptor + server-composed scope (one `eq` per concrete parent variable, omitted per `-` wildcard)/soft-delete + `ORDER BY`/`LIMIT` → in-memory SQLite), with the in-memory `filtering` matcher pinned against SQLite | wired |
| `CreateShipment`  | `resourceid` (generate), generated `ShipperResourceName` (parse parent) + `ShipmentResourceName` (validated `new` + infallible `Display`), `fieldbehavior` (reflective REQUIRED validation of all six fields — endpoints + four pickup/delivery timestamps — into one AIP-193 response, the `aip-rs` sentinel rewritten to the service domain by the boundary layer), `requestid` (AIP-155 idempotent replay), `preview` (AIP-163 `validate_only` gate) | wired |
| `ListShipments`   | `pagination` (`Page::parse` preamble: checksum guard + cursor token + `SizeLimits` size policy, then `fetch_limit`/`split_overfetch` drive the store overfetch probe whose seek is a `Predicate::tuple_gt` over the key tie-break, and mint the follow-on token; read through the generated `PageRequest` impl) + `filtering`/`aip-sql` (filter declarations derived from the `Shipment` descriptor + server-composed scope/soft-delete → in-memory SQLite) | wired |
| `BatchGetSites`   | generated `SiteResourceName` (parse each name to its typed key) + per-name primary-key `get_site` lookups in request order, with the AIP-231 whole-batch-`NOT_FOUND` default (any missing site fails the batch) | wired |
| `IAMPolicy.GetIamPolicy` / `SetIamPolicy` | `iam` (Member validation + structural read-modify-write: dedupe/normalise, `etag` optimistic concurrency, conditions⟹version-3) over a decomposed SQLite policy store (iam-go's `iam_policy_bindings` schema), with `version` recovered on read by `policy::canonical_version` (the invariant `validate` enforces, read backwards) | wired |
| `IAMPolicy.TestIamPermissions` | `iam` + the opt-in cel-backed `eval` adapter (`aip::iam::eval`): role→permission expansion via an example-owned catalogue built on `aip::iam::RoleSet` (the library ships the mechanism, never role definitions), Member matching through `aip::iam::grant_admits`, Condition evaluation against a `RequestContext` whose `request.time` defaults to now | wired |
| `BatchCreateShippers` | `lro` (AIP-151 / AIP-233): `Operation<M, R>` packs progress `metadata` + the created shippers `response` into `Any` by descriptor; a `tokio` task steps `set_metadata` → `succeed`, honouring a `CancelOperation` flag with the terminal `cancel`; `OperationName` mints the flat name; `preview` (AIP-163 `validate_only` → `Operation::validated`, empty name). Reuses the `CreateShipper` pipeline per shipper | wired |
| `Operations.GetOperation` / `ListOperations` / `WaitOperation` / `CancelOperation` / `DeleteOperation` | `lro`: `OperationName` (validate), the `not_found` shaping, `WaitTimeout` (default/cap, the block is the server's), `into_inner`/`from_inner` round-tripping the stored `Operation`. AIP-160 filtering + full `pagination` over operations deferred (ADR-0015) | wired |
| `GetSite` / `UpdateSite` / `DeleteSite`, `GetShipment` / `UpdateShipment` / `DeleteShipment` | the same primitives | `Unimplemented` |
