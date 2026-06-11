# AIP-193 error details on the tonic Status mapping

Each crate maps its `Error` to a `tonic::Status` behind a `tonic` feature.
AIP-193 requires more than a flat `INVALID_ARGUMENT` message: every error
response **must** carry a machine-readable `ErrorInfo`, and validation failures
**should** carry a `BadRequest` with field violations. The mapping below is the
shared shape every error variant slots into, built with
[`tonic-types`](https://docs.rs/tonic-types) (`ErrorDetails` + `StatusExt`),
pulled in by each crate's `tonic` feature.

## The mapping

Each error maps to `INVALID_ARGUMENT` with:

- An **`ErrorInfo`** on *every* error (the MUST): a machine-readable `reason`, a
  `domain`, and the error's dynamic values as `metadata`.
- A **`BadRequest` field violation** *only* where the error names a request
  field path — a field-mask path, an `order_by` field. A resource name, resource
  ID, page token, or filter expression is an opaque value the library validates
  without knowing which request field carried it; those get an `ErrorInfo` only.

## Conventions

- **`domain` defaults to `aip-rs` and is service-configurable** (#118). The
  library cannot know the deploying service, so standalone use stamps the stable
  library-scoped `aip-rs`. But AIP-193's domain is "typically the registered
  service name", and a deployment that lets `aip-rs` reach the wire presents two
  domains to its clients — the library's identity leaks. So each crate's
  `Error` carries, behind `tonic`, an inherent
  `into_status_with_domain(self, domain) -> tonic::Status` holding the mapping,
  and `From<Error>` delegates to it at the `aip-rs` default. The override
  carries the domain *only*, never the `reason`: a service needing its own
  reason raises its own check through `aip-validation`'s `Validator`, which
  takes both from the caller — that is the boundary between a library error
  re-domained and a service error.
- **`reason` is UPPER_SNAKE_CASE, prefixed by the AIP area** —
  `RESOURCE_NAME_*`, `RESOURCE_ID_*`, `PAGE_TOKEN_*`, `FIELD_MASK_*`,
  `ORDER_BY_*`, `FILTER_*`, `IAM_*`, … — so the `(reason, domain)` pair stays
  unique across crates sharing one domain. Per AIP-193, a `reason` matches
  `[A-Z][A-Z0-9_]+[A-Z0-9]`.
- **`metadata` carries the error's dynamic values.** AIP-193 requires that any
  request-specific information in `Status.message` also appear in `metadata`,
  so a machine actor never parses prose. Discrete values get their own
  snake_case key (`segment`, `index`, `path`, …); a free-form diagnostic with no
  discrete value is mirrored under `detail`.
- **Validators accumulate.** The reflective `validate_required` /
  `validate_required_with_mask` collect *every* missing REQUIRED path
  (`Error::RequiredFields`), mapping to one `BadRequest` violation per path —
  matching `aip-validation`'s wire shape, so swapping a hand-rolled `Validator`
  for the reflective validator changes no response.

## Why per-crate, not a shared error crate

The crates are deliberately independent (ADR-0001): each owns its `Error`, so
each owns its mapping. The convention above — not a shared type — is what keeps
the mapping consistent; the `ERROR_DOMAIN` constant and the
`into_status_with_domain` method are duplicated per crate rather than
introducing a common dependency just to share them.

## Consequences

- The mapping stays behind `tonic`; default builds pull in neither `tonic` nor
  `tonic-types`.
- Clients key on stable `(reason, domain)` pairs, so `Status.message` text may
  evolve without breaking them (AIP-193's changing-error-messages rule).
- New error variants must pick a prefixed `reason`, decide whether they name a
  field path, and put dynamic values in `metadata`.
- A deploying service is expected to re-stamp library errors with its own
  domain (`.map_err(|e| e.into_status_with_domain(DOMAIN))`), presenting one
  domain across its whole error surface; the freight example uses
  `freight.example.com` everywhere. The `into_status_with_domain` rollout is
  incremental — crates gain the method as they are touched
  (`aip-fieldbehavior` first); `From<Error>` at the `aip-rs` default remains
  correct standalone behaviour throughout.
