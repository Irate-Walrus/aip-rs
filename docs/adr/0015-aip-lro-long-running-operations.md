# aip-lro: the AIP-151 Operation state machine as a primitive, execution left to the caller

Status: proposed — needs human sign-off before implementation (issue #101).

AIP-151 long-running operations is the largest absent subsystem, and one neither
`aip-go` ships in-tree nor `iam-go` does more than stub. "Add LRO" decomposes the
same way every other crate did against the one spine aip-rs holds: each crate
**parses and validates** a convention into a native value, then leaves *execution*
to the caller — `aip-filtering` type-checks a **Filter** but never evaluates it
(ADR-0003); the core depends on no datastore (ADR-0005); `aip-iam` owns the
**Policy** structure but ships the authorization decision as an opt-in adapter
(ADR-0010). So the question is not "implement the `Operations` service" but "which
parts of LRO are parse/validate primitives, and where does the caller take over."

We add an `aip-lro` crate whose **core** owns the *state* layer of a
`google.longrunning.Operation` — the **Operation state** machine and its
transitions, the **Operation name** grammar, the `WaitOperation` **Wait timeout**
policy, and the AIP-193 error shaping — and leaves *running the work*, *persisting
the **Operation***, *minting names*, and *serving the `Operations` service* to the
caller. The `google.longrunning` types live in `aip-proto` (ADR-0011). The line is
the same one ADR-0003/0005/0010 draw; LRO sits on it exactly.

## What is in scope

- **The `Operation<M, R>` Typed facade over a Dynamic core (ADR-0009).** A
  `google.longrunning.Operation` carries its **Operation metadata** and
  **Response** as `google.protobuf.Any`. Packing a *typed* message into an `Any`
  needs that message's type URL, which only `prost_reflect::ReflectMessage`
  carries — a bare prost message does not know its own `full_name`. So the
  headline interface is generic over `M: ReflectMessage` (metadata) and
  `R: ReflectMessage` (response): the facade packs/unpacks the `Any` fields so a
  caller never hand-writes `type.googleapis.com/…` — the exact "deep module the
  library is missing" signal ADR-0009 names, the one that killed the example's
  `reflect.rs`. It layers on a still-public **Dynamic core** over
  `prost_types::Any` (the JSON/gateway escape hatch and the test surface,
  ADR-0006). This is the same Any-pack-by-descriptor move ADR-0012 makes for a
  **Change event**'s `resource`.
- **The three-state, terminal-once **Operation state** machine.** Derived from the
  wire fields exactly as `aip-softdelete`'s `State` is derived from `delete_time`:
  **Pending** (`!done`), **Succeeded** (`done` + **Response**), **Failed** (`done`
  + error). Transitions guard "must be **Pending**" (else `OPERATION_ALREADY_DONE`)
  and are terminal: `succeed(&R)`, `fail(Status)`, `cancel()` (sugar for
  `fail` with a `CANCELLED` `google.rpc.Status` — not a fourth state), and
  `set_metadata(&M)` (Pending-only). `into_inner()` / `from_inner(wire)` round-trip
  the `google.longrunning.Operation` so a caller's store persists the bytes and
  rebuilds the facade; `from_inner` validates the invariant (`done` XOR-implies
  exactly one result field) so a corrupt or foreign stored op is
  `OPERATION_MALFORMED`, not a silent bad state.
- **The validate-only (AIP-163) terminal constructor.** AIP-151 lets a
  `validate_only` request return a *done* **Operation** whose `name` may be empty,
  so the server keeps no state. `Operation::validated(&R)` is the only path to an
  empty name (`pending`/`from_inner` keep it required), encoding the fiddly
  "empty name is legal only when already-done" rule rather than leaving each
  server to re-derive it.
- **The **Operation name** grammar.** Parsed over `aip-resourcename` /
  `aip-resourceid`: an optional parent **Resource name**, the `operations`
  **Collection ID**, and a caller-minted **Operation ID**. Flat
  (`operations/{op}`) is the AIP-151 default; parent-scoped
  (`workspaces/{w}/operations/{op}`) serves multi-tenant servers (the downstream
  VIGIL requirement). The library validates the name and the id shape but **never
  mints the id** — no clock or RNG in the core, the same refusal ADR-0012 makes
  for **Change event** ids.
- **The `WaitOperation` **Wait timeout** policy.** The default the server picks
  when `timeout` is unset and the max it caps a client's timeout to — the LRO
  analog of **Size limits** (AIP-158), reflection-free and runtime-free. The
  *blocking* itself is the caller's.
- **AIP-193 error shaping behind the `tonic` feature.** The `OPERATION_*` prefix
  joins ADR-0007's list: `OPERATION_NAME_INVALID` (INVALID_ARGUMENT),
  `OPERATION_NOT_FOUND` (NOT_FOUND, a constructor a handler raises on a store miss,
  the name in `metadata` — the `aip-iam` AIP-211-helper move),
  `OPERATION_ALREADY_DONE` (FAILED_PRECONDITION), `OPERATION_MALFORMED` (INTERNAL),
  `OPERATION_WAIT_TIMEOUT_INVALID` (INVALID_ARGUMENT), and `OPERATION_ABORTED`
  (ABORTED, a shaping constructor a server fires under its own parallel-operation
  policy). Every value here is opaque or resource-name-shaped, so each error
  carries an `ErrorInfo` only, never a `BadRequest` field violation (ADR-0007). A
  `fail_status(tonic::Status)` convenience under the `tonic` feature lets a caller
  drop an aip-`Error` into `Operation.error` without hand-converting to
  `google.rpc.Status`.

## What is out of scope (a decision, not a gap)

- **No store trait.** A durable, cross-process **Operation** store is the caller's
  — `freight-server` over its map/SQLite, VIGIL over sqlx. The library defines no
  trait: an async `OperationStore` would pull a runtime contract into the crate,
  the first crack in the line `aip-iam` (the policy store is freight's, ADR-0010)
  and `aip-events` (no bus, no tokio, ADR-0012) both hold. `into_inner` /
  `from_inner` give durability without the trait; only the caller knows its
  runtime and error type.
- **No `Operations` service.** The crate ships no tonic service — `freight-server`
  and VIGIL generate the `Operations` server trait from the longrunning proto
  extern-pathed onto `aip_proto`, exactly as they generate `IAMPolicy`. The crate
  hands each of the five handlers the validated values and errors it needs.
- **No execution, no bus, no tokio, no clock.** Running the work, the
  `WaitOperation` block, **Operation** expiry (AIP-151's ~30-day sweep needs a
  clock + store scan), and id-minting are all the caller's.
- **Cancel-requested and parallel-operation policy are caller execution state.**
  `CancelOperation` is best-effort: it does not flip `done`; the **Operation**
  stays **Pending** until execution stops, then lands **Failed**/`CANCELLED`. That
  "cancel asked, work winding down" intermediate has no field in the wire
  **Operation** and no AIP-blessed home, so it is the caller's state (a flag in its
  store/task), not a fourth **Operation state**. Likewise the AIP-151
  parallel-operation rejection: the crate exposes the `ABORTED` shaping, but
  *deciding* to deny a parallel op needs the store, so the policy is the caller's.
- **No List filter Declaration.** `ListOperations` pagination is plain
  `aip-pagination` reuse. Filtering Operations on `done` and on `metadata.*` paths
  is deferred: a typed `metadata.*` **Declaration** is the same per-type machinery
  `aip-events` builds for `resource.*` (ADR-0012), gated on the in-memory matcher
  (#92). Recorded as a future slice — left out until demand appears, the
  scope discipline ADR-0010 took with `Role::from_static`.

## The aip-proto change

`google.longrunning` is generated into `aip-proto` behind a new `longrunning`
feature, per-area-gated like `iam` / `cel` (ADR-0011). Its imports resolve through
the existing pipeline: `google.protobuf.{Any, Empty, Duration}` → `prost-types`,
`google.api` → `aip_proto::google::api`. The one new edge: `Operation.error` is a
`google.rpc.Status`, which `aip-proto` does not generate. The workspace already
fixes `google.rpc` → `::tonic_types` (the freight `buf.gen.yaml` block), so the
`longrunning` feature adds `extern_path=.google.rpc=::tonic_types` — making
`tonic-types` a (non-optional, `default-features = false`) dependency of the
feature. That enforces **one `google.rpc.Status`** across the error stack, the
ADR-0011 "one `Policy`" move; generating a second `Status` in `aip-proto` would
collide with the `tonic_types::Status` the error mapping already uses.

## Considered Options

- **A value-based crate; the caller hand-packs `Any`** — drops the `prost-reflect`
  dependency, but leaves the type-URL ergonomics (the actual pain) caller-side and
  re-raises the deep-module signal ADR-0009 names. Rejected for the core; the
  Dynamic core preserves the hand-packed path for JSON/gateway callers.
- **A crate-owned `OperationStore` trait** (what a downstream consumer first asked
  for) — convenient, but an async trait drags a runtime contract into the crate and
  breaks the no-tokio line ADR-0005/0010/0012 hold. Rejected; the caller owns the
  trait, the correct owner of its runtime and error type.
- **Generate a second `google.rpc.Status` in `aip-proto`** (a `rpc` feature) —
  self-contained, but two `Status` types collide with the error stack's
  `tonic_types::Status` and break the "one type" invariant ADR-0011 enforces.
  Rejected for the extern-path.
- **An in-crate `Operations` tonic service** (batteries-included) — saves each
  server the trait generation, but couples `aip-lro` to a tonic server runtime and
  contradicts the `aip-iam` precedent (the service is freight's). Rejected; the
  opt-in path stays open if the freight pattern proves worth standardizing, as
  `aip-iam`'s `eval` did.
- **Model cancel-requested in the library** (a synthetic state or metadata field)
  — true tri-state cancellation, but it has no wire representation, would not
  survive the durable round-trip, and invents a dialect no client reads. Rejected;
  it is caller execution state.

## Consequences

- `aip-lro` is a **reflective crate**: `prost-reflect` is a hard dependency and the
  audience narrows to descriptor-generating consumers (the accepted ADR-0009
  trade), unlike the pure-string `aip-resourceid` / `aip-requestid`.
- A new opt-in `lro` umbrella feature, **off by default** while the slice matures
  (like `iam`, `events`, `sql`); the proto types ride the `aip-proto`
  `longrunning` feature and the error mapping the shared `tonic` feature, so a
  default build pulls in neither `prost-reflect`-for-LRO nor `tonic`.
- `OPERATION_*` joins ADR-0007's `reason` prefixes under the shared `aip-rs`
  domain sentinel, rewritten at the boundary by `aip-errordomain`.
- CONTEXT.md gains **Operation**, **Operation name**, **Operation ID**,
  **Operation state**, **Operation result**, **Response**, **Operation metadata**,
  **Cancellation**, and **Wait timeout** (done).
- **Risk to verify in implementation:** cross-pool `Any`-packing — an `Operation`
  lives in `aip-proto`'s descriptor pool while its `M`/`R` carry descriptors from
  the consumer's pool. Packing is by value plus a type-URL string, so no shared
  descriptor is resolved across pools; this is the same cross-crate-reflection risk
  ADR-0011 verified for a freight message embedding a `Policy`.

## Sequencing — tracer bullet first

1. **Now:** this ADR, for sign-off (stage 1 of #101).
2. **The crate:** `aip-lro` — the `Operation<M, R>` facade + Dynamic core, the
   **Operation state** machine, **Operation name**, the validate-only constructor,
   **Wait timeout**, and the `OPERATION_*` errors — plus the `aip-proto`
   `longrunning` feature.
3. **freight-server wiring:** `BatchCreateShippers` gets an AIP-233 + AIP-151 LRO
   shape on the worked **Shipper** resource — `response` a
   `BatchCreateShippersResponse { repeated Shipper }`, `metadata` a
   `BatchCreateShippersMetadata { created, total }`. A real tokio task creates the
   shippers with a small delay, stepping `set_metadata` then `succeed`;
   `CancelOperation` sets freight's cancel flag, the task checks it and calls
   `cancel()`; `WaitOperation` blocks on a per-op `tokio::sync::Notify`. The
   `Operations` service is served alongside `FreightService` and `IAMPolicy`;
   `validate_only` returns `Operation::validated(&response)` through the existing
   `aip-preview` gate (#130). README gains grpcurl for start → poll → done and
   start → cancel; the status table is updated.
