# AIP-193 error details on the tonic Status mapping

Each crate already maps its `Error` to a `tonic::Status` behind a `tonic`
feature. AIP-193 requires more than a flat `INVALID_ARGUMENT` message: every
error response **must** carry a machine-readable `ErrorInfo`, and validation
failures **should** carry a `BadRequest` with field violations. So the
`From<Error> for tonic::Status` impls attach those standard details, and this is
the shared mapping every later error variant slots into.

## The mapping

For each crate, `From<Error> for tonic::Status` produces `INVALID_ARGUMENT` with:

- An **`ErrorInfo`** on *every* error (AIP-193's MUST), carrying a machine-readable
  `reason`, a `domain`, and the error's dynamic values as `metadata`.
- A **`BadRequest` field violation** *only* where the error names a field path —
  a field-mask path (`FIELD_MASK_UNKNOWN_PATH`) or an `order_by` field
  (`ORDER_BY_UNKNOWN_FIELD`). A resource name, resource ID, page token, or filter
  expression is an opaque value the library validates without knowing which
  request field carried it, so those get an `ErrorInfo` only.

Built with [`tonic-types`](https://docs.rs/tonic-types) (`ErrorDetails` +
`StatusExt`), pulled in alongside `tonic` by each crate's `tonic` feature.

## Conventions for future error variants

- **`domain` is `aip-rs`** for every library error. The library does not know the
  deploying service, so it uses a stable library-scoped domain rather than a
  service name. A *service* (e.g. the example server) uses its own domain for the
  checks no primitive covers.
- **`reason` is UPPER_SNAKE_CASE, prefixed by the AIP area** — `RESOURCE_NAME_*`,
  `RESOURCE_ID_*`, `PAGE_TOKEN_*`, `FIELD_MASK_*`, `ORDER_BY_*`, `FILTER_*` — so
  the `(reason, domain)` pair is unique across crates that share the one domain.
  Per AIP-193, a `reason` must match `[A-Z][A-Z0-9_]+[A-Z0-9]`.
- **`metadata` carries the error's dynamic values.** AIP-193 requires that any
  request-specific information appearing in `Status.message` also appear in
  `metadata`, so a machine actor never has to parse the prose. Discrete values get
  their own key (the offending `segment`, `index`, `path`, `character`,
  `position`, …); a free-form diagnostic with no discrete value is mirrored under
  the `detail` key. Keys are snake_case and match `[a-z][a-zA-Z0-9-_]+`.
- **A `BadRequest` is added only when the error identifies a request field path.**

## Why per-crate, not a shared error crate

The crates are deliberately independent (ADR-0001): each owns its `Error`, so each
owns its `From<Error>` impl. The convention above — not a shared type — is what
keeps the mapping consistent. The small `domain` constant is duplicated per crate
rather than introducing a common dependency just to share a string.

## Consequences

- The mapping stays behind the `tonic` feature; default builds are unaffected and
  pull in neither `tonic` nor `tonic-types`.
- Clients get stable, machine-readable identifiers, so `Status.message` text may
  evolve without breaking them (AIP-193's changing-error-messages rule).
- Future error variants must pick a prefixed `reason`, decide whether they name a
  field path, and put dynamic values in `metadata`.

## Amendment (#118): the `ErrorInfo` domain is service-configurable

The first Conventions bullet above — **"`domain` is `aip-rs` for every library
error"** — is revised. `aip-rs` is now the *default*, not a mandate: a deploying
service may re-stamp any library error with its own AIP-193 domain, so the
[`aip-validation`](../../crates/aip-validation) `Validator` escape hatch and the
library validators finally speak one domain instead of standing apart.

**Why the original was too strict.** AIP-193 says `ErrorInfo.domain` is "typically
the registered service name." A deployment that lets `aip-rs` reach the wire
presents *two* domains to its clients — `aip-rs` for library-caught errors (a bad
page token, an unknown `order_by`, a missing REQUIRED field) and its own name for
checks it hand-rolled. The library's internal identity leaks, and a client keying
on `(reason, domain)` sees an inconsistent surface. The `Validator` already took
the caller's domain; only the library validators were stuck at `aip-rs`. This
amendment closes that gap: the service owns one domain across its whole error
surface, and `aip-rs` is the standalone default for a primitive used *without* a
deploying service.

**Signature — `into_status_with_domain`, uniform across crates.** Every crate that
hard-codes `ERROR_DOMAIN` gains, behind its `tonic` feature, an inherent method on
its `Error`, and `From<Error>` delegates to it at the default domain:

```rust
#[cfg(feature = "tonic")]
impl Error {
    /// Maps to the AIP-193 `Status`, stamping `domain` into the `ErrorInfo`
    /// instead of the default `aip-rs`. The `reason` and `metadata` are unchanged.
    pub fn into_status_with_domain(self, domain: impl Into<String>) -> tonic::Status { /* the mapping */ }
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self { err.into_status_with_domain(ERROR_DOMAIN) } // default "aip-rs"
}
```

- **The method holds the mapping; `From` delegates.** The per-crate AIP-193 mapping
  moves into `into_status_with_domain`; `From<Error>` becomes the same behavior at
  `ERROR_DOMAIN`. No mapping logic is duplicated.
- **Uniform by convention, not a shared type.** Like `ERROR_DOMAIN` itself, the
  method is repeated per crate rather than hoisted into a shared crate — preserving
  the ADR-0001 independence and the "convention, not a shared type" stance above.
- **Domain only, never `reason`.** The override carries the domain; the library
  keeps its stable, machine-readable `reason` (`FIELD_REQUIRED`, `PAGE_TOKEN_*`, …),
  and the prefixes keep `(reason, domain)` unique in the service's namespace. A
  service that needs its *own* reason raises its *own* check through `Validator`,
  which owns both `domain` and `reason` — that is the boundary between a library
  error *re-domained* and a service error.
- **`aip-validation` is exempt.** It never hard-coded `ERROR_DOMAIN`;
  `Validator::new(domain, reason)` already takes both from the caller.

**Reflective validators now accumulate.** `validate_required` /
`validate_required_with_mask` stop bailing on the first missing field and collect
*every* missing REQUIRED path: `Error::RequiredField { path }` becomes
`Error::RequiredFields { paths: Vec<String> }`, mapping to one `BadRequest`
violation per path plus a single comma-joined `fields` metadata key — matching
`aip-validation`'s wire shape, so a service swapping a hand-rolled `Validator` for
the reflective validator gets the same response. (`validate_immutable_not_changed`
keeps bail-on-first for now; accumulating the update path is a parallel future
change.)

**The `Validator` escape hatch is unchanged — and stops being abused.** `Validator`
remains the surface for checks no primitive covers: cross-field rules, policy
checks, and attaching a `BadRequest` field path to an otherwise opaque-value error
(e.g. `validate_shipper_name` naming the `parent` / `name` field on a resource-name
pattern mismatch). Its use in `create_site` / `create_shipment` for hand-rolled
presence checks on REQUIRED fields was duplication that had already drifted
(`create_shipment` checked 2 of its 6 REQUIRED fields); those become reflective
`validate_required` calls.

**Consequence for the freight example.** The example collapses to a single domain —
`freight.example.com` — across its whole surface, demonstrating the recommended
production pattern (a real service does not leak `aip-rs` to clients):

- Every library-error site re-stamps via
  `.map_err(|e| e.into_status_with_domain(SERVICE_DOMAIN))?` — `aip-requestid`,
  `aip-fieldbehavior`, `aip-ordering`, `aip-iam`, and the rest — replacing the bare
  `.map_err(Status::from)`.
- `create_site` / `create_shipment` drop their hand-rolled `Validator`s for
  `validate_required` + the override, fixing the drift (all REQUIRED fields are now
  covered reflectively).
- `validate_shipper_name`'s `Validator` stays — the legitimate escape hatch —
  already on `SERVICE_DOMAIN`.
- The six tests asserting `info.domain == "aip-rs"` (request-id conflict and
  malformed, the two `validate_required` paths, the unknown `order_by`, and the IAM
  denial) flip to `"freight.example.com"`. The one test already asserting
  `"freight.example.com"` stays, now sourced from the reflective validator rather
  than the hand-rolled `Validator`. Each crate's own unit tests keep asserting the
  `aip-rs` *default*, which is unchanged.

This is a design amendment; the implementation and the freight adoption are the
blocked follow-up (#118).
