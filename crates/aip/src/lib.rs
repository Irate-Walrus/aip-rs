//! aip — a Rust SDK for Google's API Improvement Proposals (AIP).
//!
//! Umbrella crate that re-exports the per-feature crates. All features are on by
//! default; disable default features and pick only what you need:
//!
//! ```toml
//! aip = { version = "0.1", default-features = false, features = ["resourcename"] }
//! ```

#[cfg(feature = "resourcename")]
pub use aip_resourcename as resourcename;

#[cfg(feature = "resourceid")]
pub use aip_resourceid as resourceid;

/// AIP-155 request identification (issue #94) — default-on via the `requestid`
/// feature: validate a `request_id` and name the idempotency [`Replay`] contract
/// a server enforces over its own cache of seen ids.
///
/// [`Replay`]: aip_requestid::Replay
#[cfg(feature = "requestid")]
pub use aip_requestid as requestid;

#[cfg(feature = "pagination")]
pub use aip_pagination as pagination;

#[cfg(feature = "fieldmask")]
pub use aip_fieldmask as fieldmask;

#[cfg(feature = "ordering")]
pub use aip_ordering as ordering;

#[cfg(feature = "filtering")]
pub use aip_filtering as filtering;

#[cfg(feature = "fieldbehavior")]
pub use aip_fieldbehavior as fieldbehavior;

/// AIP-154 content etags (issue #93) — default-on via the `etag` feature:
/// [`compute`](aip_etag::compute) digests a resource's content
/// (excluding the etag field and OUTPUT_ONLY noise) into an optimistic-concurrency
/// token, and [`check`](aip_etag::check) verifies a client's etag before
/// a write. Generalizes the IAM Policy etag to any resource.
#[cfg(feature = "etag")]
pub use aip_etag as etag;

/// AIP-164 soft delete / AIP-165 purge (issue #96) — default-on via the
/// `softdelete` feature: the soft-delete [`State`](aip_softdelete::State) rules
/// ([`is_visible`](aip_softdelete::is_visible) /
/// [`check_visible`](aip_softdelete::check_visible) for `show_deleted` gating,
/// [`check_deleted`](aip_softdelete::check_deleted) for the undelete
/// precondition) and the AIP-165 purge contract
/// ([`PurgeMode`](aip_softdelete::PurgeMode) /
/// [`require_filter`](aip_softdelete::require_filter) /
/// [`purge_result`](aip_softdelete::purge_result)).
#[cfg(feature = "softdelete")]
pub use aip_softdelete as softdelete;

/// AIP-163 `validate_only` preview gate (issue #130) — default-on via the
/// `preview` feature: [`commit_unless`](aip_preview::commit_unless) runs a
/// mutation's commit only for a real write, skipping the store write and the
/// AIP-155 idempotency record on a preview while the handler still returns the
/// would-be resource.
#[cfg(feature = "preview")]
pub use aip_preview as preview;

/// Field-violation accumulator (issue #60): collects per-field validation
/// failures into one AIP-193 error. Default-on via the `validation` feature.
#[cfg(feature = "validation")]
pub use aip_validation as validation;

/// Resource-annotation reflection (issue #61) — default-on via the `reflect`
/// feature: parse a [`ResourceType`](aip_reflect::ResourceType), iterate the
/// `google.api.resource` descriptors in a file/package, and validate a message's
/// `google.api.resource_reference` fields.
#[cfg(feature = "reflect")]
pub use aip_reflect as reflect;

/// IAM primitives (ADR-0010) — opt-in via the non-default `iam` feature: parse and
/// validate the `google.iam.v1` identity vocabulary (Member / Role / Permission).
#[cfg(feature = "iam")]
pub use aip_iam as iam;

/// SQL adapter (ADR-0008) — opt-in via the non-default `sql` feature, since it is
/// not part of the parse/validate core (ADR-0005).
#[cfg(feature = "sql")]
pub use aip_sql as sql;
