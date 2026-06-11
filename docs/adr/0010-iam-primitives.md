# IAM as parse/validate primitives, with the authorization decision as an opt-in adapter

"Add IAM" is really several separable things, and aip-rs has one consistent
spine to hold them against: every crate **parses and validates** a convention
into a native value, then leaves *execution* to the caller — `aip-filtering`
type-checks a **Filter** but never evaluates it (ADR-0003); the core depends on
no datastore (ADR-0005). So the question is not "add IAM" but "which parts of
IAM are parse/validate primitives, and where does the caller take over."

We add an `aip-iam` crate whose **core** owns the *structural* layer of
`google.iam.v1` (the package AIP-213 blesses for re-use) plus the AIP-211
authorization-error shape, and which ships the **authorization decision** —
role→permission expansion and IAM **Condition** evaluation — as an *opt-in*
`cel`-backed adapter, never in the core. That keeps the core on the same line
ADR-0003 draws for filter evaluation while still giving servers real IAM
behaviour, exactly as `aip-sql` keeps SQL transpilation opt-in (ADR-0005/0008).

## What is in scope

- **Member / Role / Permission parsing** — pure string work over the
  `google.iam.v1` member grammar (`user:`, `serviceAccount:`, `group:`,
  `domain:`, `allUsers`, `allAuthenticatedUsers`), the role-name forms, and the
  `{service}.{resource}.{verb}` permission form. The same shape of job as
  `aip-resourceid`/`aip-resourcename`, and like them proto-free.
- **The `google.iam.v1.Policy` structure + structural ops** — add/remove a
  **Member** from a **Binding**, normalise/dedupe, the `etag`
  read-modify-write dance, and the *conditions ⟺ policy version 3* invariant.
  The generated types ride behind an optional feature (since ADR-0011, from
  `aip-proto`).
- **AIP-211 error shaping** behind the `tonic` feature — the canonical
  non-leaking `PERMISSION_DENIED` (*"Permission '{p}' denied on resource '{r}'
  (or it might not exist)"*) plus the NOT_FOUND-via-parent helper. The direct
  analog of ADR-0007's AIP-193 details, and the highest-value piece: the
  non-leaking semantics are fiddly and easy to get wrong.
- **Conditions are CEL — general CEL, not the AIP-160 subset.** An IAM
  **Condition** (`google.type.Expr`) is arbitrary CEL over IAM's own
  environment (`resource.*`, `request.*`, time helpers), so `aip-filtering`'s
  CEL interop — which maps only the filter subset — is the wrong tool and would
  reject most real conditions. To compile or evaluate a Condition we reach for
  the `cel` crate (cel-rust) behind a feature; absent that, the core treats a
  Condition as an opaque validated string.
- **The authorization decision, as the opt-in `iam-eval` adapter.** Given a
  **Policy**, a principal's memberships, the caller's role→permission
  catalogue, and a request context: allow/deny, with conditions evaluated.
  Execution layer, so it lives behind the opt-in `iam-eval` umbrella feature
  (forwarding to `aip-iam`'s `eval` feature) depending on `cel`, never in a
  default build. It is what makes `TestIamPermissions` actually decide rather
  than stub.

## What is out of scope (a decision, not a gap)

- **Evaluation in the core.** The decision and condition evaluation ship only
  as the opt-in adapter above — the architectural line, not a feasibility one.
- **Authentication / credentials** (the `auth/411x` AIPs: ADC, JWT, service
  account keys, mTLS). Client-side credential acquisition is a different
  library entirely; a non-goal for aip-rs.

## Non-reflective, unlike the other proto-touching crates

IAM operates on a *fixed* schema (`google.iam.v1.Policy`), so there is no
descriptor pool to thread and no Typed facade / Dynamic core split — ADR-0009
does not apply. The parsing primitives are pure strings; the Policy-structure
layer operates on the generated types directly.

## Sequencing — tracer bullet first (all landed)

A thin end-to-end slice before breadth, as ADR-0008 drove a **Filter** into
SQLite: first the crate with Member/Role/Permission parsing wired into the
umbrella as `aip::iam`; then the Policy structural ops with
`GetIamPolicy`/`SetIamPolicy` in `freight-server` over a policy store keyed by
**Resource name** (#64, #65); then the `cel`-backed `iam-eval` adapter with
`TestIamPermissions` deciding through it, and the AIP-211 helpers (#66, #68,
#67). The `aip-filtering` CEL bridge is *not* used — see the conditions point.

## Considered Options

- **One IAM crate that also decides** (batteries-included evaluator) — most
  convenient, but drags CEL execution into the core and breaks the
  no-evaluation invariant ADR-0003/0005 rely on. Rejected for the core; adopted
  as the opt-in adapter instead — the `aip-sql`-shaped answer.
- **No crate — document the convention, servers hand-roll** — cheapest, but
  every server re-writes member parsing and the AIP-211 non-leaking error,
  exactly the "deep module the library is missing" signal ADR-0009 names.
- **Fold IAM into `aip-resourcename`** (members/roles are name-shaped) —
  conflates a distinct AIP area's vocabulary; kept separate so the glossary
  terms (**Member**, **Role**, **Permission**, **Policy**, **Binding**) stay
  unambiguous (CONTEXT.md).

## Consequences

- An opt-in `iam` feature on the umbrella, off by `default` (like `sql`); the
  proto layer rides a further feature and the error mapping the shared `tonic`
  feature, so default builds pull in neither `prost` nor `tonic`.
- New AIP-193 `reason` prefix `IAM_*` under the shared default domain
  (ADR-0007).
- The opt-in umbrella feature `iam-eval` (forwarding to `aip-iam`'s `eval`)
  pulls the `cel` crate; it is never in `default`, so parse/validate-only
  users never compile `cel`.
- `freight-server` gains an `IAMPolicy` service and a resource-name-keyed
  policy store, with `TestIamPermissions` deciding *through* the `iam-eval`
  adapter — demonstrating execution as an opt-in layer over the core.
