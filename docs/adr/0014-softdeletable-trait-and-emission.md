# A `SoftDeletable` trait, emitted resource-anchored from `delete_time`

`aip-softdelete` gains a `SoftDeletable` trait so a handler passes a resource
straight to the visibility checks instead of restating
`State::from_deleted(resource.delete_time.is_some())` at every call site, and the
codegen plugin (ADR-0011 / ADR-0013) gains a fourth emission — `impl
SoftDeletable` — keyed on a resource carrying a `delete_time`. It is off by
default behind a `softdelete` plugin flag.

## What changes

- **`aip-softdelete` grows one blessed path.** A `SoftDeletable` trait with a
  single `soft_delete_state(&self) -> State`, plus a blanket
  `impl<T: SoftDeletable> From<&T> for State`. The three check functions
  (`is_visible` / `check_visible` / `check_deleted`) are generalized from a bare
  `State` to `impl Into<State>`. A call site reads
  `check_visible(&shipper, show_deleted, &name)` — no `State::from_deleted(…)`,
  and **no trait import**, because the blanket `From` fires on its own. Bare-`State`
  callers (the crate's own tests) compile unchanged through the reflexive
  `From<State> for State`. `State::from_deleted` stays public: it is what a trait
  impl is written with.
  - The trait returns the crate's own `State`, **not**
    `Option<&prost_types::Timestamp>`. That keeps `aip-softdelete` value-based and
    dependency-free — the "no proto, no reflection" promise of ADR-0005 holds —
    and writes the rule "`delete_time` stamped ⇒ `Deleted`" once, in the impl.
- **`aip-reflect`'s `ResourceDescriptor` gains `message_name` and
  `has_delete_time`.** `message_name` is the simple name of the message that
  carries the `google.api.resource` annotation — also its prost struct, which the
  impl is named on — and is `None` for a file-level `resource_definition` (no
  owning message). `has_delete_time` is true iff that message carries a singular,
  non-repeated `google.protobuf.Timestamp delete_time` field.
- **`aip-codegen` emits the impl.** `CratePaths` gains a `softdelete` path. For
  each resource with `has_delete_time` and a `message_name`, `generate` writes
  `impl ::aip_softdelete::SoftDeletable for <Message>` reading
  `self.delete_time.is_some()`. The impl is **resource-anchored but
  pattern-independent**: a patternless soft-deletable resource still earns it,
  emitted into the same per-proto `<proto>.aip.rs`, fully path-qualified and
  mounted in the message module (ADR-0013's one mount rule).
- **A fourth plugin flag, `softdelete`, default off.** Parsed in the existing
  `key=value` `opt:` style with the same hard-error on an unknown key or a
  non-bool value. A disabled flag zeroes `has_delete_time` on every resource, so
  the pure generator never sees a flag (mirroring `pagination` / `ordering`).
  Enabling it makes `aip-softdelete` a direct dependency of the consumer.

## Why

- **The boilerplate was the issue.** The example server wrote the same
  `check_visible(State::from_deleted(r.delete_time.is_some()), …)` incantation
  five times, and every consumer with a soft-deletable resource would write it
  for every handler that touches visibility. A trait moves the
  `delete_time`-to-`State` step from each call site into one generated impl.
- **Emission is resource-anchored, not field-shaped.** `SoftDeletable` is
  semantically a resource property (AIP-164), so the predicate is a
  `google.api.resource`-annotated message *that also* carries a `delete_time` —
  not pure field shape (the rule ADR-0013 uses for request traits). A pure
  field-shape rule would misfire on, say, an audit-log message that happens to
  carry a `delete_time` but is not a resource.
- **A wrong-typed `delete_time` is a silent no-impl, never an error.** A field
  named `delete_time` that is not a `google.protobuf.Timestamp` (or is repeated)
  yields `has_delete_time = false`: the resource simply gets no impl, and the
  missing `SoftDeletable` surfaces at the generic call site. This carries
  ADR-0013's near-miss precedent into the resource predicate.
- **`impl Into<State>`, not a second function name.** Generalizing the existing
  functions keeps one name per rule forever; the pre-0.1 signature change is
  free, and the reflexive `From` keeps every bare-`State` caller compiling.

## Considered Options

- **A `State::of_delete_time(Option<&Timestamp>)` helper instead of a trait.**
  Rejected: it would pull `prost_types` into the crate, breaking the
  dependency-free promise, and still leaves a per-call-site conversion. The trait
  returns the crate's own `State` and hides the conversion in one impl.
- **Pure field-shape emission (a message with a `delete_time` field).** Rejected:
  `SoftDeletable` is a resource property; field shape alone would emit the impl on
  non-resource messages that merely carry a `delete_time`.
- **Hand-written impls in the example.** Rejected by the example-server rule
  (ADR-0006): the feature is not done until the demo generates the impl. The
  plugin emits it; the example carries none.

## Consequences

- **Amends ADR-0011 and ADR-0013.** A fourth plugin output and flag, sharing
  ADR-0013's `use`-free, mount-in-message-module format and its
  flag-zeroes-the-bool convention. The wrapper golden fixtures are re-blessed to
  carry the new impl, and a negative golden fixture (a resource without
  `delete_time`) pins that no impl is emitted.
- **freight-server follow-up** (per the example-server rule): `softdelete=true`
  in `buf.gen.yaml`, the regenerated `.aip.rs` for Shipper / Site / Shipment
  committed, all five `check_visible` / `is_visible` / `check_deleted` call sites
  converted to pass the resource directly, and no hand-written impls. Because the
  example routes generation through the `aip` umbrella (`aip_crate=aip`), the
  emitted `::aip::softdelete::SoftDeletable` rides the existing `aip` dependency —
  no new direct dep.
- **CONTEXT.md gains "Soft-deletable".**
