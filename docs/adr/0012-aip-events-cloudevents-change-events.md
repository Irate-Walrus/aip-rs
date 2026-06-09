# aip-events: resource change events as CloudEvents, with transport left to the caller

Status: proposed — needs human sign-off before implementation (issue #103).

No published AIP standardizes an event bus, so "AIP-compliant events" has to mean
following the precedents Google actually ships: resource-centric change events
(Pub/Sub notification configs, Cloud Asset feeds, Kubernetes watch), delivered as
**CloudEvents with protobuf payloads** (Eventarc). We adopt that combination
literally: a **Change event** is one resource's create/update/delete/undelete,
carried in the standard `io.cloudevents.v1.CloudEvent` envelope with a crate-owned
typed data payload. The crate owns the event *shape*, the **Update mask** diff,
and **Subscription** matching; **transport is the caller's** (in-process channel,
NATS, Pub/Sub), on the same parse/validate-don't-execute line as ADR-0003/0005/0010.

## The event shape

The envelope is the canonical CloudEvents proto, consumed as a managed BSR
dependency (`buf.build/cloudevents/cloudevents`) through the ADR-0011 buf
pipeline — not a lookalike. A custom envelope would type every field nicely but
forfeit the interop that makes CloudEvents worth choosing: existing
transports, brokers, and tooling already route on its context attributes.

Context attributes are fixed as follows:

- **`type`** — derived per resource from its AIP-123 resource type plus the
  **Change kind** verb: `{service}/{Type}` ⇒
  `{service}.{type_snake_case}.v1.{created|updated|deleted|undeleted}` (e.g.
  `freight-example.einride.tech/Shipper` ⇒
  `freight-example.einride.tech.shipper.v1.created`). The `v1` is the *event
  payload schema* version, exactly as in google-cloudevents type strings. The
  Rust `ChangeKind` enum formats and parses the verb suffix; the change kind is
  **not** duplicated inside the payload — the type string is the single source
  of truth, as transports route on it.
- **`subject`** — the **Resource name**. This is also the designated ordering
  key; preserving per-subject order is the transport's job, the crate merely
  names the key.
- **`source`** — `//{Service name}`, so that `source` + `subject` composes to
  the AIP-122 **Full resource name**.
- **`id`** — publisher-supplied; the crate validates non-emptiness but never
  generates ids (no clock or RNG in the core). Documented convention: when the
  mutating request carried an AIP-155 `request_id`, set `id` to
  `{request_id}/{subject}` — a retried request then reproduces the same
  `(source, id)` pair and CloudEvents-level dedup fires; the per-subject suffix
  keeps batch and purge requests (one request, many resources) unique. Without
  a `request_id` the publisher must supply something unique per event.
- **`time`** — the event time.

The `data` field carries a crate-owned typed payload, the google-cloudevents
pattern (typed data message per event family) rather than stringly extension
attributes — a `FieldMask` should stay a `FieldMask`:

```proto
package aip.events.v1;

message ResourceChangeData {
  google.protobuf.Any resource = 1;          // post-change state, Any-packed Typed message
  google.protobuf.FieldMask update_mask = 2; // changed paths; UPDATED only
  string etag = 3;                           // post-change etag/revision, if the resource has one
  reserved 4;                                // prior_resource — known non-goal, kept additive
}
```

**Post-state only.** `resource` is the post-change state; `update_mask` says
which paths changed. Prior state (the Cloud Asset feed `prior_asset` pattern)
is recorded as a non-goal with field number 4 reserved: it would double payload
and diff cost and drag "how are `prior.` paths declared" into the filter
surface. The known limitation is accepted: a subscriber can filter "status *is*
DELAYED and `update_mask` touched `status`", but not "status went from ACTIVE
to DELAYED".

**DELETED carries the last-known state.** Soft delete (#96) genuinely has a
post-state (the resource with its state flipped) and maps to `…deleted`;
undelete maps to `…undeleted`. A hard delete or AIP-165 purge has no
post-state, so `resource` carries the final pre-delete state — the Kubernetes
watch convention. This is the one place `resource` is not literally post-state;
it keeps resource-field filters meaningful across every change kind instead of
silently never matching deletions.

## The Update mask diff

`aip-fieldmask` grows a `diff` / `diff_dynamic` pair (Typed facade / Dynamic
core, ADR-0009): two messages of one **Descriptor** in, the **Field mask** of
changed paths out. It lives there, not in `aip-events`, because it is the
inverse of applying an **Update mask**, walks the same paths, and is useful
beyond events (computing a write's effective mask). Granularity mirrors what an
AIP-134 update mask can meaningfully express: descend through singular message
fields to the deepest changed path (`origin_site.lat_lng.latitude`); repeated
and map fields are atomic — any element change yields the bare field path.

## The Subscription surface

A **Subscription** is a standing AIP-160 **Filter** over Change events, checked
against a **Declaration** the crate builds:

- envelope **Identifiers** — `type`, `source`, `subject`, `time`, and
  `update_mask` filterable through the **Has operator**
  (`update_mask:"display_name"`);
- when the Subscription names a resource type, that resource's fields under a
  `resource.` prefix, declared mechanically from its **Descriptor**
  (`resource.display_name = "ACME"`).

Resource fields are *not* flattened to the top level: every AIP resource has a
`name` field, so flattening collides with envelope vocabulary immediately and
forces shadowing rules. A Subscription that names no resource type may filter
on envelope identifiers only. Checking is `aip-filtering`'s; evaluation is the
in-memory matcher (#92), which this crate is hard-blocked on.

## The boundary: no bus in the crate

`aip-events` owns event construction (diff ⇒ envelope), the Declaration
builder, and Subscription matching (`matches(&event) -> bool`). It ships **no
bus, no streams, no tokio**: the issue lists the in-process channel as a
*caller* transport alongside NATS and Pub/Sub, and ADR-0005 draws the same
line. `freight-server` carries the worked example — a tokio broadcast bus
(~tens of lines) feeding a watch-style server-streaming RPC that takes a
`filter` — and is the template for any other transport. Delivery semantics
(buffering, lag, redelivery) are therefore explicitly the transport's contract,
never the crate's.

## Sequencing

1. **Now:** this ADR, for sign-off (stage 1 of #103).
2. **Blocked on #92:** the crate — `aip.events.v1` proto, `aip-fieldmask::diff`,
   Declaration builder, Subscription matching.
3. **Stage 2:** freight-server wiring — handlers publish to an in-process bus,
   watch RPC streams filtered events, grpcurl demo in the README; soft delete
   (#96) maps to DELETED/UNDELETED and request_id (#94) feeds the `id`
   convention as each lands.

## Considered Options

- **A custom envelope message instead of `io.cloudevents.v1.CloudEvent`** —
  typed fields everywhere (a real enum for the change kind, FieldMask at the top
  level), but a private dialect: nothing existing routes it. Rejected for
  interop; the typed fields move into the data payload instead.
- **Bare `Any` data plus string extension attributes** for mask/etag — no
  authored proto, but the FieldMask degrades to a comma-joined string
  convention. Rejected.
- **Generic fixed type strings** (`aip.resource.v1.created`) — simpler
  derivation, but type-based routing then cannot distinguish resources and
  every subscriber needs a filter. Rejected for the per-resource derived form,
  the Eventarc precedent.
- **Crate-generated UUID event ids** — convenient, but retries then produce
  distinct ids and the AIP-155 dedup tie-in the issue asks for is lost (and the
  core would grow an RNG dependency). Rejected.
- **Optional prior state in v1** — true transition filtering, at double the
  payload/diff cost plus a `prior.` declaration question. Deferred; field
  number reserved.
- **The in-process bus in the crate** (as core, or `eval`-style opt-in
  feature) — saves adopters boilerplate but pulls a runtime dependency and
  delivery semantics into the crate's contract. Rejected for v1; the opt-in
  feature remains open as a later step if the freight-server pattern proves
  worth standardizing, the same path `aip-iam`'s `eval` took.

## Consequences

- aip-rs authors its **first own `.proto`** (`aip.events.v1.ResourceChangeData`):
  buf lint and breaking checks now guard a schema we publish, not just consume,
  and the ADR-0011 `message_name` enumeration extends to it.
- New pinned BSR dependency on `buf.build/cloudevents/cloudevents`.
- `aip-fieldmask`'s public API grows `diff` / `diff_dynamic`.
- The crate is **hard-blocked on #92** for Subscription matching; shape and
  diff work could land first behind the proposed flag, but the issue's
  acceptance criteria require the matcher.
- A new opt-in `events` feature on the umbrella, off by default while the slice
  matures (like `sql`, `iam`).
- CONTEXT.md gains **Change event**, **Change kind**, and **Subscription**
  (done alongside this ADR).
