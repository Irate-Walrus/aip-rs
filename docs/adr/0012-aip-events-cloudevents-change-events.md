# aip-events: resource change events as CloudEvents, with transport left to the caller

Status: proposed — needs human sign-off before implementation (issue #103).

No published AIP standardizes an event bus, so "AIP-compliant events" means
following the precedents Google ships: resource-centric change events (Pub/Sub
notification configs, Cloud Asset feeds, Kubernetes watch), delivered as
**CloudEvents with protobuf payloads** (Eventarc). A **Change event** is one
resource's create/update/delete/undelete, carried in the standard
`io.cloudevents.v1.CloudEvent` envelope with a crate-owned typed data payload.
The crate owns the event *shape*, the **Update mask** diff, and **Subscription**
matching; **transport is the caller's** (in-process channel, NATS, Pub/Sub), on
the same parse/validate-don't-execute line as ADR-0003/0005/0010.

## The event shape

The envelope is the canonical CloudEvents proto, a managed BSR dependency
(`buf.build/cloudevents/cloudevents`) through the ADR-0011 pipeline — not a
lookalike, because the interop (transports, brokers, tooling routing on its
context attributes) is what makes CloudEvents worth choosing. Attributes:

- **`type`** — derived per resource from its AIP-123 resource type plus the
  **Change kind** verb: `{service}/{Type}` ⇒
  `{service}.{type_snake_case}.v1.{created|updated|deleted|undeleted}`, the
  google-cloudevents form (`v1` versions the event payload schema). The
  `ChangeKind` enum formats/parses the verb; the change kind is **not**
  duplicated in the payload — transports route on the type string.
- **`subject`** — the **Resource name**; also the designated ordering key
  (preserving per-subject order is the transport's job).
- **`source`** — `//{Service name}`, so `source` + `subject` composes to the
  AIP-122 **Full resource name**.
- **`id`** — publisher-supplied; the crate validates non-emptiness but never
  generates ids (no clock or RNG in the core). Convention: when the mutating
  request carried an AIP-155 `request_id`, set `id` to
  `{request_id}/{subject}` — a retry reproduces the same `(source, id)` pair so
  CloudEvents-level dedup fires, and the per-subject suffix keeps batch/purge
  requests (one request, many resources) unique.
- **`time`** — the event time.

The `data` field carries a crate-owned typed payload (the google-cloudevents
pattern — a `FieldMask` should stay a `FieldMask`, not a stringly extension
attribute):

```proto
package aip.events.v1;

message ResourceChangeData {
  google.protobuf.Any resource = 1;          // post-change state, Any-packed Typed message
  google.protobuf.FieldMask update_mask = 2; // changed paths; UPDATED only
  string etag = 3;                           // post-change etag/revision, if the resource has one
  reserved 4;                                // prior_resource — known non-goal, kept additive
}
```

**Post-state only.** Prior state (the Cloud Asset `prior_asset` pattern) is a
recorded non-goal with field 4 reserved: it would double payload and diff cost
and drag "how are `prior.` paths declared" into the filter surface. Accepted
limitation: a subscriber can filter "status *is* DELAYED and `update_mask`
touched `status`", not "status went from ACTIVE to DELAYED".

**DELETED carries the last-known state.** Soft delete (#96) genuinely has a
post-state and maps to `…deleted`; undelete to `…undeleted`. A hard delete or
AIP-165 purge has no post-state, so `resource` carries the final pre-delete
state — the Kubernetes watch convention, and the one place `resource` is not
literally post-state. It keeps resource-field filters meaningful across every
change kind instead of silently never matching deletions.

## The Update mask diff

`aip-fieldmask` grows a `diff` / `diff_dynamic` pair (Typed facade / Dynamic
core, ADR-0009): two messages of one **Descriptor** in, the **Field mask** of
changed paths out. It lives there, not in `aip-events`, because it is the
inverse of applying an **Update mask** and is useful beyond events. Granularity
mirrors what an AIP-134 mask can express: descend through singular message
fields to the deepest changed path; repeated and map fields are atomic.

## The Subscription surface

A **Subscription** is a standing AIP-160 **Filter** over Change events, checked
against a **Declaration** the crate builds:

- envelope **Identifiers** — `type`, `source`, `subject`, `time`, and
  `update_mask` filterable through the **Has operator**
  (`update_mask:"display_name"`);
- when the Subscription names a resource type, that resource's fields under a
  `resource.` prefix, declared mechanically from its **Descriptor**
  (`resource.display_name = "ACME"`). Not flattened to the top level: every AIP
  resource has a `name`, so flattening collides with envelope vocabulary.

Checking is `aip-filtering`'s; evaluation is the in-memory matcher (#92), which
this crate is hard-blocked on.

## The boundary: no bus in the crate

`aip-events` owns event construction (diff ⇒ envelope), the Declaration
builder, and Subscription matching (`matches(&event) -> bool`). It ships **no
bus, no streams, no tokio** — ADR-0005's line. `freight-server` carries the
worked example — a tokio broadcast bus (~tens of lines) feeding a watch-style
server-streaming RPC that takes a `filter` — and is the template for any other
transport. Delivery semantics (buffering, lag, redelivery) are explicitly the
transport's contract, never the crate's.

## Sequencing

1. **Now:** this ADR, for sign-off (stage 1 of #103).
2. **Blocked on #92:** the crate — `aip.events.v1` proto, `aip-fieldmask::diff`,
   Declaration builder, Subscription matching.
3. **Stage 2:** freight-server wiring — handlers publish to an in-process bus,
   watch RPC streams filtered events, grpcurl demo in the README; soft delete
   (#96) maps to DELETED/UNDELETED and request_id (#94) feeds the `id`
   convention.

## Considered Options

- **A custom envelope message** — typed fields everywhere, but a private
  dialect nothing existing routes. Rejected for interop; the typed fields move
  into the data payload instead.
- **Bare `Any` data plus string extension attributes** — the FieldMask degrades
  to a comma-joined string convention. Rejected.
- **Generic fixed type strings** (`aip.resource.v1.created`) — type-based
  routing then cannot distinguish resources. Rejected for the per-resource
  derived form, the Eventarc precedent.
- **Crate-generated UUID event ids** — retries then produce distinct ids,
  losing the AIP-155 dedup tie-in, and the core grows an RNG dependency.
  Rejected.
- **Optional prior state in v1** — true transition filtering at double the
  payload/diff cost plus a `prior.` declaration question. Deferred; field
  number reserved.
- **The in-process bus in the crate** (core or `eval`-style opt-in) — saves
  boilerplate but pulls a runtime dependency and delivery semantics into the
  crate's contract. Rejected for v1; the opt-in feature remains open if the
  freight-server pattern proves worth standardizing, the path `aip-iam`'s
  `eval` took.

## Consequences

- aip-rs authors its **first own `.proto`** (`aip.events.v1.ResourceChangeData`):
  buf lint and breaking checks now guard a schema we publish, and the ADR-0011
  `message_name` enumeration extends to it.
- New pinned BSR dependency on `buf.build/cloudevents/cloudevents`.
- `aip-fieldmask`'s public API grows `diff` / `diff_dynamic`.
- The crate is **hard-blocked on #92** for Subscription matching.
- A new opt-in `events` feature on the umbrella, off by default while the slice
  matures (like `sql`, `iam`).
- CONTEXT.md gains **Change event**, **Change kind**, and **Subscription**.
