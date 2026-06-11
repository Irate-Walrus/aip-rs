//! AIP-163 `validate_only`: the preview gate for a mutating request.
//!
//! A `validate_only` request asks the server to run a mutation as far as it can
//! *without persisting it* — validate fully, build the would-be resource, and
//! return it, but commit nothing. This crate names that gate so every consumer
//! shares one contract instead of re-deriving it from a four-line `if`.
//!
//! # The contract
//!
//! The flag gates only the commit; it never forks the validation path:
//!
//! - **Validation runs unconditionally.** The full pipeline (REQUIRED fields,
//!   resource-name and etag checks, any accumulator) runs *before* this gate, so
//!   a request that would fail fails identically — same error, same AIP-193
//!   details — with or without the flag.
//! - **A preview commits nothing.** It skips the store write **and** the AIP-155
//!   idempotency record, so a later real create still mints a new resource: the
//!   preview reserved no id and left no replay entry.
//! - **The would-be resource is still returned.** The handler returns the
//!   resource it *would* have committed, system-assigned id and etag already
//!   minted, so the caller sees exactly what a real write would produce.
//! - **One line per handler.** `validate_only` stays a single
//!   [`commit_unless`] call wrapping the commit, not a second branch through the
//!   handler.
//!
//! # Value-based
//!
//! The gate reasons over the `bool` alone — it pulls in no datastore, no
//! protobuf, and no reflection. The caller owns the commit closure (the store
//! write and idempotency record); this primitive owns only the decision to run
//! it.
//!
//! ```
//! # use aip_preview as preview;
//! # struct Resource;
//! # fn validate() {}
//! # fn build_resource() -> Resource { Resource }
//! # fn persist(_: &Resource) {}
//! # let validate_only = true;
//! validate();
//! let resource = build_resource();
//! preview::commit_unless(validate_only, || persist(&resource));
//! // `resource` is returned either way; only `persist` is gated.
//! ```
//!
//! See <https://google.aip.dev/163>.

/// Run `commit` unless the request is an AIP-163 `validate_only` preview.
///
/// Reads as the gate it names — *commit unless preview*: returns `Some(commit())`
/// for a real write, or `None` for a preview, where the commit is skipped.
///
/// Validation and the would-be resource are built *before* this call; `commit`
/// is just the persistence — the store write and the AIP-155 idempotency record.
/// On a preview that closure never runs, so the store and idempotency cache stay
/// untouched while the handler still returns the resource it would have
/// committed. The `Option` carries the commit's own result for stores whose write
/// yields one (a row id, the persisted handle); a side-effect-only commit returns
/// `Option<()>`, which the caller ignores.
pub fn commit_unless<T>(validate_only: bool, commit: impl FnOnce() -> T) -> Option<T> {
    if validate_only {
        None
    } else {
        Some(commit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn real_write_runs_the_commit() {
        let committed = Cell::new(false);
        commit_unless(false, || committed.set(true));
        assert!(committed.get());
    }

    #[test]
    fn preview_skips_the_commit() {
        let committed = Cell::new(false);
        commit_unless(true, || committed.set(true));
        // A preview persists nothing: the store write and idempotency record both
        // stay un-run.
        assert!(!committed.get());
    }

    #[test]
    fn real_write_returns_the_commit_value() {
        // The commit's own result (e.g. a store handle) flows back through `Some`.
        assert_eq!(commit_unless(false, || 7), Some(7));
    }

    #[test]
    fn preview_returns_none_without_running_the_commit() {
        let committed = Cell::new(false);
        let result = commit_unless(true, || {
            committed.set(true);
            7
        });
        assert_eq!(result, None);
        assert!(!committed.get());
    }
}
