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
- **`metadata` carries the error's structured dynamic values** (the offending
  segment, index, path, character, position, …), not free-form prose. Keys are
  snake_case and match `[a-z][a-zA-Z0-9-_]+`.
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
