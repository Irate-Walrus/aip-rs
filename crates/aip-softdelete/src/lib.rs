//! AIP-164 soft delete + AIP-165 criteria-based delete (purge): the state rules
//! for soft-deleting, reading, and undeleting a resource, plus the purge
//! confirmation contract — over any resource carrying a `delete_time`.
//!
//! This generalizes the example server's ad-hoc soft delete (a `delete_time`
//! stamp and a `delete_time IS NULL` list predicate) into a primitive. Like the
//! rest of the core it *owns the state rules and their errors*, not the storage:
//! when a resource's `delete_time` is stamped, where the `delete_time IS NULL`
//! filter is applied, and how rows are removed on a purge stay the caller's — the
//! primitive decides what those states *mean* and which AIP-193 error a bad
//! transition is.
//!
//! # Soft delete (AIP-164)
//!
//! A soft delete marks a resource as deleted (stamps `delete_time`) instead of
//! removing it, so it can be recovered with an undelete. The two state rules:
//!
//! - **Read visibility.** A soft-deleted resource is hidden unless the caller
//!   opts in with `show_deleted`. [`is_visible`] is the boolean a `List` filters
//!   on; [`check_visible`] is the `Get`/`Delete` form that turns a hidden
//!   resource into a [`NOT_FOUND`](Error::Deleted) — the caller cannot tell a
//!   hidden resource from one that never existed. (AIP-164 lets `Get` return a
//!   soft-deleted resource by default; a server that prefers to gate `Get` on
//!   `show_deleted` uses [`check_visible`] to do so.)
//! - **Undelete precondition.** Undelete operates only on a soft-deleted
//!   resource; calling it on a live one is [`ALREADY_EXISTS`](Error::NotDeleted)
//!   (AIP-164's Errors section). [`check_deleted`] enforces this.
//!
//! # Purge (AIP-165)
//!
//! A purge is a criteria-based, permanent delete of every resource matching a
//! `filter`. Its safety contract:
//!
//! - The `filter` is **required** ([`require_filter`]) — a purge must be scoped.
//! - `force` gates whether it actually deletes ([`PurgeMode`]): unset ⇒
//!   [`Preview`](PurgeMode::Preview), returning a count and a [capped
//!   sample](PURGE_SAMPLE_MAX) of what *would* be deleted; set ⇒
//!   [`Execute`](PurgeMode::Execute), performing the deletion.
//! - [`purge_result`] shapes the `(count, sample)` from the matched names per
//!   that rule (sample only in a preview), which the caller maps onto its
//!   `purge_count` / `purge_sample` response fields.
//!
//! # State, not storage
//!
//! The primitive is value-based: it reasons over a [`State`] derived from whether
//! a `delete_time` is present, so it pulls in no datastore and no reflection. A
//! caller holding a typed resource implements [`SoftDeletable`] once — or has the
//! codegen plugin emit it — and then passes the resource itself
//! (`check_visible(&shipper, …)`); the check functions take `impl Into<State>`,
//! so a bare [`State`] still works for callers that compute it directly. A
//! SQL-backed list keeps expressing the visibility rule as its own `delete_time
//! IS NULL` predicate rather than going through this crate.
//!
//! See <https://google.aip.dev/164> and <https://google.aip.dev/165>.
#![cfg_attr(docsrs, feature(doc_cfg))]

/// The soft-delete state of a resource: whether its `delete_time` is stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The resource is not deleted (no `delete_time`).
    Live,
    /// The resource is soft-deleted (its `delete_time` is stamped).
    Deleted,
}

impl State {
    /// The state implied by whether a `delete_time` is present: `Deleted` when it
    /// is set, `Live` otherwise. A caller holding a typed resource passes
    /// `resource.delete_time.is_some()`.
    pub fn from_deleted(deleted: bool) -> Self {
        if deleted {
            State::Deleted
        } else {
            State::Live
        }
    }

    /// Whether the resource is soft-deleted.
    pub fn is_deleted(self) -> bool {
        matches!(self, State::Deleted)
    }
}

/// A resource that knows its own soft-delete [`State`] — the call-site sugar
/// that lets a handler pass the resource itself to [`is_visible`] /
/// [`check_visible`] / [`check_deleted`] instead of spelling out
/// `State::from_deleted(resource.delete_time.is_some())` at every visibility
/// check.
///
/// The trait returns this crate's own [`State`], **not** an
/// `Option<&prost_types::Timestamp>`: that keeps the crate value-based and
/// dependency-free (no proto, no reflection — see the crate docs and ADR-0005),
/// and means the one rule "`delete_time` stamped ⇒ `Deleted`" is written once,
/// in the impl, rather than re-derived at each call site.
///
/// A blanket [`From<&T>`](State) turns any `&impl SoftDeletable` into a [`State`],
/// so the check functions take `impl Into<State>` and a call site needs **no
/// trait import** — the conversion fires on its own:
///
/// ```
/// use aip_softdelete::{check_visible, SoftDeletable, State};
///
/// struct Shipper {
///     delete_time: Option<()>,
/// }
///
/// impl SoftDeletable for Shipper {
///     fn soft_delete_state(&self) -> State {
///         State::from_deleted(self.delete_time.is_some())
///     }
/// }
///
/// let shipper = Shipper { delete_time: None };
/// // No `State::from_deleted(...)` at the call site, and no `State` import needed.
/// check_visible(&shipper, false, "shippers/acme").unwrap();
/// ```
///
/// The codegen plugin emits this impl for every `google.api.resource`-annotated
/// message carrying a `google.protobuf.Timestamp delete_time` field, so a
/// consumer hand-writes none (ADR-0014).
pub trait SoftDeletable {
    /// The resource's soft-delete [`State`]: [`Deleted`](State::Deleted) when its
    /// `delete_time` is stamped, [`Live`](State::Live) otherwise.
    fn soft_delete_state(&self) -> State;
}

/// Any `&impl SoftDeletable` converts to its [`State`], so a resource reference
/// is accepted directly wherever a check function takes `impl Into<State>`. A
/// bare [`State`] still converts through the reflexive `From<State> for State`,
/// so callers passing a `State` (the crate's own tests) are unaffected.
impl<T: SoftDeletable> From<&T> for State {
    fn from(resource: &T) -> State {
        resource.soft_delete_state()
    }
}

/// Errors produced by the soft-delete / purge state rules.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// A soft-deleted resource was read without `show_deleted` set, so it is
    /// hidden (AIP-164). The message deliberately reads as a plain
    /// not-found — the caller must not be able to tell a hidden resource from one
    /// that never existed. Maps to `NOT_FOUND`.
    #[error("resource `{name}` not found")]
    Deleted {
        /// The resource name that was requested.
        name: String,
    },
    /// Undelete was called on a resource that is not soft-deleted (AIP-164): the
    /// resource is already live, so there is nothing to recover. Maps to
    /// `ALREADY_EXISTS`.
    #[error("resource `{name}` is not deleted")]
    NotDeleted {
        /// The resource name that was requested.
        name: String,
    },
    /// A purge request carried an empty `filter`. AIP-165 requires a purge to be
    /// scoped by a filter, so an unscoped one is rejected rather than treated as
    /// "delete everything". Maps to `INVALID_ARGUMENT`.
    #[error("a purge filter is required")]
    FilterRequired,
}

/// Whether a resource in `state` is visible to a request carrying the given
/// `show_deleted` (AIP-164): a live resource always is; a soft-deleted one only
/// when `show_deleted` is set.
///
/// A `List` filters its results on this boolean; a `Get`/`Delete` uses
/// [`check_visible`], which turns a hidden resource into a [`NOT_FOUND`
/// error](Error::Deleted) instead.
///
/// `state` is `impl Into<State>`, so a handler passes either a bare [`State`] or,
/// via the blanket [`From<&T>`](State), a `&impl `[`SoftDeletable`] resource
/// directly (e.g. `is_visible(&shipper, show_deleted)`).
pub fn is_visible(state: impl Into<State>, show_deleted: bool) -> bool {
    show_deleted || !state.into().is_deleted()
}

/// AIP-164 read visibility for a `Get`/`Delete`: succeed when the resource is
/// [visible](is_visible), otherwise report it hidden.
///
/// `name` is the requested resource name, carried on the error so the AIP-193
/// mapping can put it in `ErrorInfo.metadata`.
///
/// # Errors
///
/// [`Error::Deleted`] when the resource is soft-deleted and `show_deleted` is
/// false — a hidden resource the caller is not allowed to distinguish from a
/// never-existing one (`NOT_FOUND` on the wire).
///
/// `state` is `impl Into<State>`: a bare [`State`] or a `&impl `[`SoftDeletable`]
/// resource (e.g. `check_visible(&shipper, show_deleted, &name)`).
pub fn check_visible(state: impl Into<State>, show_deleted: bool, name: &str) -> Result<(), Error> {
    if is_visible(state, show_deleted) {
        Ok(())
    } else {
        Err(Error::Deleted {
            name: name.to_owned(),
        })
    }
}

/// AIP-164 undelete precondition: the resource must be soft-deleted.
///
/// `name` is the requested resource name, carried on the error for the AIP-193
/// `ErrorInfo.metadata`.
///
/// # Errors
///
/// [`Error::NotDeleted`] when the resource is live — there is nothing to recover,
/// so the undelete is `ALREADY_EXISTS` on the wire.
///
/// `state` is `impl Into<State>`: a bare [`State`] or a `&impl `[`SoftDeletable`]
/// resource (e.g. `check_deleted(&shipper, &name)`).
pub fn check_deleted(state: impl Into<State>, name: &str) -> Result<(), Error> {
    if state.into().is_deleted() {
        Ok(())
    } else {
        Err(Error::NotDeleted {
            name: name.to_owned(),
        })
    }
}

/// The maximum number of resource names a purge preview's sample should carry.
/// AIP-165: "a good rule of thumb is 100", and it is a maximum — a server may
/// return fewer.
pub const PURGE_SAMPLE_MAX: usize = 100;

/// Whether an AIP-165 purge actually deletes, decided by the request's `force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeMode {
    /// `force` was unset: return the count and a [capped sample](PURGE_SAMPLE_MAX)
    /// of the resources that *would* be deleted, deleting nothing.
    Preview,
    /// `force` was set: perform the deletion.
    Execute,
}

impl PurgeMode {
    /// The mode the request's `force` flag selects: `Execute` when set, otherwise
    /// the safe `Preview`.
    pub fn from_force(force: bool) -> Self {
        if force {
            PurgeMode::Execute
        } else {
            PurgeMode::Preview
        }
    }

    /// Whether this is a [`Preview`](PurgeMode::Preview) (no deletion happens).
    pub fn is_preview(self) -> bool {
        matches!(self, PurgeMode::Preview)
    }
}

/// AIP-165 requires a purge to be scoped by a filter.
///
/// # Errors
///
/// [`Error::FilterRequired`] when `filter` is empty (`INVALID_ARGUMENT` on the
/// wire). A non-empty filter — including the `"*"` wildcard a service may accept
/// for "everything" — passes; validating the filter's *syntax* is the caller's
/// job (e.g. via `aip-filtering`).
pub fn require_filter(filter: &str) -> Result<(), Error> {
    if filter.is_empty() {
        Err(Error::FilterRequired)
    } else {
        Ok(())
    }
}

/// Shape an AIP-165 purge result from the resource names matching the filter.
///
/// `count` is always how many matched. `sample` is the first
/// [`PURGE_SAMPLE_MAX`] names in a [`Preview`](PurgeMode::Preview) and **empty**
/// in an [`Execute`](PurgeMode::Execute) — AIP-165 populates the sample only when
/// `force` is false. The caller maps `count` / `sample` onto its `purge_count` /
/// `purge_sample` response fields.
pub fn purge_result(mode: PurgeMode, matched: &[String]) -> (usize, Vec<String>) {
    let sample = if mode.is_preview() {
        matched.iter().take(PURGE_SAMPLE_MAX).cloned().collect()
    } else {
        Vec::new()
    };
    (matched.len(), sample)
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to a canonical gRPC code with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `SOFT_DELETE_*` / `PURGE_*` `reason` +
    /// `domain` (`aip-rs`) and the error's dynamic values as `metadata`.
    ///
    /// A hidden soft-deleted read ([`Deleted`](Error::Deleted)) maps to
    /// `NOT_FOUND`; an undelete of a live resource
    /// ([`NotDeleted`](Error::NotDeleted)) maps to `ALREADY_EXISTS` (the AIP-164
    /// contract); a missing purge filter ([`FilterRequired`](Error::FilterRequired))
    /// maps to `INVALID_ARGUMENT`. A resource name is an opaque value rather than
    /// a named request field path, so no `BadRequest` is attached (matching
    /// `aip-etag`).
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (code, reason, metadata): (tonic::Code, &str, HashMap<String, String>) = match &err {
            Error::Deleted { name } => (
                tonic::Code::NotFound,
                "SOFT_DELETE_NOT_FOUND",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
            Error::NotDeleted { name } => (
                tonic::Code::AlreadyExists,
                "SOFT_DELETE_NOT_DELETED",
                HashMap::from([("name".to_owned(), name.clone())]),
            ),
            Error::FilterRequired => (
                tonic::Code::InvalidArgument,
                "PURGE_FILTER_REQUIRED",
                HashMap::new(),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(code, message, details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_from_delete_time_presence() {
        assert_eq!(State::from_deleted(false), State::Live);
        assert_eq!(State::from_deleted(true), State::Deleted);
        assert!(!State::Live.is_deleted());
        assert!(State::Deleted.is_deleted());
    }

    /// A typed resource standing in for a generated one: `delete_time` presence
    /// is the soft-delete state, exactly as the codegen impl reads it.
    struct Shipper {
        delete_time: Option<()>,
    }

    impl SoftDeletable for Shipper {
        fn soft_delete_state(&self) -> State {
            State::from_deleted(self.delete_time.is_some())
        }
    }

    #[test]
    fn soft_deletable_resource_converts_to_its_state() {
        // The blanket `From<&T>` reads the trait, so `&resource` is an `Into<State>`.
        let live = Shipper { delete_time: None };
        let deleted = Shipper {
            delete_time: Some(()),
        };
        assert_eq!(State::from(&live), State::Live);
        assert_eq!(State::from(&deleted), State::Deleted);
    }

    #[test]
    fn check_functions_accept_the_resource_directly() {
        // No `State::from_deleted(...)` at the call site: the resource reference
        // converts on its own (the issue's whole point).
        let live = Shipper { delete_time: None };
        let deleted = Shipper {
            delete_time: Some(()),
        };

        assert!(is_visible(&live, false));
        assert!(!is_visible(&deleted, false));
        assert!(is_visible(&deleted, true));

        assert_eq!(check_visible(&live, false, "shippers/acme"), Ok(()));
        assert_eq!(
            check_visible(&deleted, false, "shippers/acme"),
            Err(Error::Deleted {
                name: "shippers/acme".to_owned()
            }),
        );

        assert_eq!(check_deleted(&deleted, "shippers/acme"), Ok(()));
        assert_eq!(
            check_deleted(&live, "shippers/acme"),
            Err(Error::NotDeleted {
                name: "shippers/acme".to_owned()
            }),
        );
    }

    #[test]
    fn live_resource_is_always_visible() {
        // show_deleted never hides a live resource.
        assert!(is_visible(State::Live, false));
        assert!(is_visible(State::Live, true));
        assert_eq!(check_visible(State::Live, false, "shippers/acme"), Ok(()));
        assert_eq!(check_visible(State::Live, true, "shippers/acme"), Ok(()));
    }

    #[test]
    fn soft_deleted_resource_is_visible_only_with_show_deleted() {
        assert!(!is_visible(State::Deleted, false));
        assert!(is_visible(State::Deleted, true));

        // Hidden without show_deleted: a NOT_FOUND carrying the requested name.
        assert_eq!(
            check_visible(State::Deleted, false, "shippers/acme"),
            Err(Error::Deleted {
                name: "shippers/acme".to_owned()
            }),
        );
        // Opted in: the soft-deleted resource is returned.
        assert_eq!(check_visible(State::Deleted, true, "shippers/acme"), Ok(()));
    }

    #[test]
    fn undelete_requires_a_deleted_resource() {
        // A soft-deleted resource can be undeleted.
        assert_eq!(check_deleted(State::Deleted, "shippers/acme"), Ok(()));
        // A live one cannot: there is nothing to recover (ALREADY_EXISTS).
        assert_eq!(
            check_deleted(State::Live, "shippers/acme"),
            Err(Error::NotDeleted {
                name: "shippers/acme".to_owned()
            }),
        );
    }

    #[test]
    fn purge_mode_follows_force() {
        assert_eq!(PurgeMode::from_force(false), PurgeMode::Preview);
        assert_eq!(PurgeMode::from_force(true), PurgeMode::Execute);
        assert!(PurgeMode::Preview.is_preview());
        assert!(!PurgeMode::Execute.is_preview());
    }

    #[test]
    fn purge_filter_is_required() {
        assert_eq!(require_filter(""), Err(Error::FilterRequired));
        // A concrete filter and the "*" wildcard both pass; syntax is not checked
        // here.
        assert_eq!(require_filter("state = \"ACTIVE\""), Ok(()));
        assert_eq!(require_filter("*"), Ok(()));
    }

    #[test]
    fn purge_preview_samples_matches_but_deletes_nothing() {
        let matched: Vec<String> = (0..150).map(|i| format!("shippers/s{i}")).collect();
        let (count, sample) = purge_result(PurgeMode::Preview, &matched);
        // The count reflects every match; the sample is capped.
        assert_eq!(count, 150);
        assert_eq!(sample.len(), PURGE_SAMPLE_MAX);
        assert_eq!(sample[0], "shippers/s0");
    }

    #[test]
    fn purge_execute_reports_count_without_a_sample() {
        let matched: Vec<String> = (0..5).map(|i| format!("shippers/s{i}")).collect();
        let (count, sample) = purge_result(PurgeMode::Execute, &matched);
        // AIP-165: the sample is populated only when force is false.
        assert_eq!(count, 5);
        assert!(sample.is_empty());
    }

    #[test]
    fn purge_preview_under_the_cap_returns_every_match() {
        let matched: Vec<String> = (0..3).map(|i| format!("shippers/s{i}")).collect();
        let (count, sample) = purge_result(PurgeMode::Preview, &matched);
        assert_eq!(count, 3);
        assert_eq!(sample, matched);
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn hidden_soft_deleted_read_maps_to_not_found_with_metadata() {
        let status: tonic::Status = Error::Deleted {
            name: "shippers/acme".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::NotFound);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "SOFT_DELETE_NOT_FOUND");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("name").map(String::as_str),
            Some("shippers/acme"),
        );
        // A resource name is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn undelete_of_a_live_resource_maps_to_already_exists() {
        let status: tonic::Status = Error::NotDeleted {
            name: "shippers/acme".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::AlreadyExists);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "SOFT_DELETE_NOT_DELETED");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("name").map(String::as_str),
            Some("shippers/acme"),
        );
    }

    #[test]
    fn missing_purge_filter_maps_to_invalid_argument() {
        let status: tonic::Status = Error::FilterRequired.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "PURGE_FILTER_REQUIRED");
        assert_eq!(info.domain, ERROR_DOMAIN);
    }
}
