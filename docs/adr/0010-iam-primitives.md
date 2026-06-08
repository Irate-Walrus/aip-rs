# IAM as parse/validate primitives, with the authorization decision as an opt-in adapter

"Add IAM" is really four separable things, and aip-rs has one consistent spine to
hold them against: every crate **parses and validates** a convention into a native
value, then leaves *execution* to the caller — `aip-filtering` type-checks a
**Filter** but never evaluates it (ADR-0003); `aip-ordering` validates paths but
the datastore sorts; the core depends on no datastore (ADR-0005). The question is
therefore not "add IAM" but "which parts of IAM are parse/validate primitives, and
where is the seam where the caller takes over."

We answer it by adding an `aip-iam` crate whose **core** owns the *structural*
layer of `google.iam.v1` (the package AIP-213 blesses for re-use) plus the AIP-211
authorization-error shape, and which ships the **authorization decision** —
role→permission expansion and IAM **Condition** evaluation — as an *opt-in*
`cel`-backed adapter rather than in the core. That keeps the parse/validate core on
the same line ADR-0003 draws for filter evaluation, while still giving servers real
IAM behaviour, exactly as `aip-sql` keeps SQL transpilation an opt-in layer
(ADR-0005/0008).

## What is in scope

- **Member / Role / Permission parsing** — pure string work over the
  `google.iam.v1` member grammar (`user:`, `serviceAccount:`, `group:`, `domain:`,
  `allUsers`, `allAuthenticatedUsers`), the role-name forms (`roles/{role}`,
  `projects/{p}/roles/{r}`, `organizations/{o}/roles/{r}`), and the
  `{service}.{resource}.{verb}` permission form. This is the strongest fit: the
  same shape of job as `aip-resourceid`/`aip-resourcename`, and like them it is
  proto-free.
- **The `google.iam.v1.Policy` structure + structural ops** — add/remove a
  **Member** from a **Binding**, normalise/dedupe bindings, the `etag`
  read-modify-write dance, and the *conditions ⟺ policy version 3* invariant. The
  generated `google.iam.v1` types ride behind an optional feature, mirroring
  `aip-filtering`'s `cel-proto` (vendored protos, `protox`, no `protoc` —
  ADR-0001).
- **AIP-211 error shaping** behind the `tonic` feature — the canonical
  `PERMISSION_DENIED` with the non-leaking message *"Permission '{p}' denied on
  resource '{r}' (or it might not exist)"*, plus the NOT_FOUND-via-parent helper.
  This is the direct analog of ADR-0007's AIP-193 details and is the highest-value
  piece, since the non-leaking semantics are fiddly and easy to get wrong.
- **Conditions are CEL — but general CEL, not the AIP-160 subset.** An IAM
  **Condition** (`google.type.Expr`) is an arbitrary CEL string over IAM's own
  environment (`resource.*`, `request.*`, time helpers), *not* an AIP-160
  **Filter**. So `aip-filtering`'s CEL interop — which maps only the filter subset
  ⇄ `v1alpha1` protos — is the wrong tool here: it would reject most real
  conditions. To *do* anything with a Condition (validate that it compiles, or
  evaluate it) we reach for the `cel` crate (the cel-rust project, formerly
  `cel-interpreter`) behind a feature; absent that the core treats a Condition as
  an opaque validated string.
- **The authorization decision, as an opt-in `cel`-backed adapter.** Given a
  **Policy**, a principal's memberships, the caller's role→permission catalogue, and
  a request context, return allow/deny — *with conditions evaluated*. This is the
  execution layer, so it lives behind an opt-in `eval` feature (or a sibling crate)
  depending on the `cel` crate, never in the parse/validate core and never in a
  default build — the same shape `aip-sql` gives SQL transpilation. It is what makes
  `TestIamPermissions` actually decide rather than stub.

## What is out of scope (recorded so it is a decision, not a gap)

- **Evaluation *in the core*.** The decision and condition evaluation never enter
  the parse/validate core or a default build; they ship only as the opt-in
  `cel`-backed adapter above. (Feasibility was never the question — the `cel` crate
  is a pure-Rust interpreter we depend on rather than build; the architectural line
  is. Even transpilation lives in a separate opt-in crate, `aip-sql`.)
- **Authentication / credentials** (the `auth/4110`–`4119` AIPs: ADC, JWT, service
  account keys, mTLS, workload identity). That is client-side credential
  acquisition — a different kind of library entirely. A non-goal for aip-rs.

## Non-reflective, unlike the other proto-touching crates

IAM operates on a *fixed* schema (`google.iam.v1.Policy`), so there is no
descriptor pool to thread and no **Typed facade / Dynamic core** split — ADR-0009
does not apply. The parsing primitives are pure strings (no proto at all, like
`aip-resourceid`); only the optional Policy-structure layer generates types, and it
operates on them directly.

## Sequencing — tracer bullet first

We land a thin end-to-end slice before breadth, the same way ADR-0008 drove a
**Filter** into SQLite:

1. **Now:** the `aip-iam` crate with Member/Role/Permission parsing (proto-free,
   `tonic` error mapping), wired into the umbrella as `aip::iam`.
2. **Slice 1 completion:** generate `google.iam.v1.Policy`, add the structural ops,
   and wire `GetIamPolicy`/`SetIamPolicy` into `freight-server` over a policy store
   keyed by **Resource name** — a **Member** string travelling request → validate →
   store → response (per CLAUDE.md, the feature is not done until the example uses
   it).
3. **Backlog:** the opt-in `cel`-backed `eval` adapter (**Condition** evaluation +
   the allow/deny decision), `TestIamPermissions` deciding *through* that adapter in
   `freight-server`, and the AIP-211 `PERMISSION_DENIED` / NOT_FOUND-via-parent
   helpers. (The `aip-filtering` CEL bridge is *not* used — see the conditions point
   above.)

## Considered Options

- **One IAM crate that also decides** (batteries-included evaluator, e.g.
  `cel`-backed). Most convenient for a server, but drags CEL *execution* into the
  core and breaks the no-evaluation invariant ADR-0003/0005 rely on. Rejected for
  the core; the `cel`-backed evaluator is instead **adopted as an opt-in adapter**
  (feature or sibling crate) — the `aip-sql`-shaped answer for IAM.
- **No crate — document the convention and let servers hand-roll Policy handling.**
  Cheapest, but every server then re-writes member parsing and the AIP-211
  non-leaking error, which is exactly the "deep module the library is missing"
  signal ADR-0009 names. Rejected.
- **Fold IAM into `aip-resourcename`** (members/roles are name-shaped). Conflates a
  distinct AIP area's vocabulary into the resource-name crate; kept separate so the
  glossary terms (**Member**, **Role**, **Permission**, **Policy**, **Binding**)
  stay unambiguous (CONTEXT.md).

## Consequences

- A new opt-in `iam` feature on the umbrella, off by `default` while the slice
  matures (like `sql`); the proto layer rides a further `iam-proto` feature and the
  error mapping the shared `tonic` feature, so default builds pull in neither
  `prost` nor `tonic`.
- New AIP-193 `reason` prefix `IAM_*` under the shared `aip-rs` domain (ADR-0007),
  unique across crates.
- A further opt-in `eval` feature (or sibling crate) pulls the `cel` crate for
  **Condition** evaluation and the allow/deny decision; it is never in `default`, so
  parse/validate-only users never compile `cel`.
- `freight-server` gains an `IAMPolicy` service and a resource-name-keyed policy
  store, and its `TestIamPermissions` decides *through* the opt-in `eval` adapter —
  demonstrating execution as an opt-in layer over the core, exactly where the
  library's parse/validate line sits.
