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

- **`domain` is the `aip-rs` sentinel, rewritten once at the boundary** (#118,
  amended by #145 — see the amendment below). The library cannot know the
  deploying service, so every error it maps stamps the stable library-scoped
  `aip-rs` through `From<Error>` — the one and only conversion. But AIP-193's
  domain is "typically the registered service name", and a deployment that lets
  `aip-rs` reach the wire presents two domains to its clients — the library's
  identity leaks. So `aip-rs` is a *sentinel* meaning "replace at the serving
  boundary": a deploying service installs the `aip-errordomain` layer, which
  rewrites it to the service's own domain once, at the edge. A service needing
  its own `reason` (not just its own domain) raises its own check through
  `aip-validation`'s `Validator`, which takes both from the caller and is left
  untouched by the layer — that is the boundary between a library error
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
the mapping consistent; the `ERROR_DOMAIN` sentinel constant and the
`From<Error>` mapping are duplicated per crate rather than introducing a common
dependency just to share them. (The `aip-errordomain` layer that rewrites the
sentinel at the boundary, #145, is *not* such a shared dependency — it carries
no `Error` type and no crate depends on it; see the amendment below and
ADR-0001.)

## Consequences

- The mapping stays behind `tonic`; default builds pull in neither `tonic` nor
  `tonic-types`.
- Clients key on stable `(reason, domain)` pairs, so `Status.message` text may
  evolve without breaking them (AIP-193's changing-error-messages rule).
- New error variants must pick a prefixed `reason`, decide whether they name a
  field path, and put dynamic values in `metadata`.
- A deploying service presents one domain across its whole error surface by
  installing the `aip-errordomain` boundary layer (see the amendment below); the
  freight example shows `freight.example.com` to every client. `From<Error>` at
  the `aip-rs` sentinel remains correct standalone behaviour — a library used
  without the layer simply shows `aip-rs`.

## Amendment (issue #145): one domain, set at the boundary — not per call site

The original design gave each crate's `Error` an inherent
`into_status_with_domain(self, domain)` and expected a service to re-stamp every
library error at the call site
(`.map_err(|e| e.into_status_with_domain(DOMAIN))`). That left the domain story
split: a service had to remember to re-stamp at *every* call site, and the ones
it missed leaked `aip-rs` to clients alongside the ones it caught — two domains
from one server. The freight example had exactly this split (re-stamped
`create_*`, leaking `order_by` / etag / IAM paths).

**Decision:** stamp the domain **once, at the serving boundary**, with a tower
layer — not at each call site.

- The library mapping is just `From<Error>` at the `aip-rs` **sentinel**. The
  inherent `into_status_with_domain` is removed; there is one conversion.
- A new crate **`aip-errordomain`** provides a tonic/tower
  `Layer` + `Service` that rewrites `grpc-status-details-bin` on the way out:
  it decodes the `google.rpc.Status`, and for any `ErrorInfo` whose `domain`
  equals the `aip-rs` sentinel, replaces it with the service's domain. It covers
  both the **headers** (a trailers-only unary error) and the **trailers** (a
  streaming error, by wrapping the response body). The rewrite is
  **sentinel-only**: a service-raised domain (a `Validator`) or any third-party
  domain passes untouched.
- The service installs it once on its builder:
  `Server::builder().layer(aip_errordomain::Layer::new(SERVICE_DOMAIN))`. The
  handlers convert library errors with a bare `?`.
- `aip-errordomain` carries no `Error` type and no aip-* dependency, so it is
  not the shared-error crate ADR-0001 rejected; it is re-exported as
  `aip::errordomain` behind the umbrella `tonic` feature.

The pre-boundary contract — `From<Error>` stamps `aip-rs` — is what the crates'
direct-call tests pin; the through-the-wire rewrite to the service domain is
proven by one freight smoke test that drives the layered service.
