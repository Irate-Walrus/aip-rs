//! AIP-211 authorization-error shaping — opt-in via the non-default `tonic` feature.
//!
//! The two helpers a server reaches for at an authorization boundary, once it has
//! *decided* a caller is not authorized:
//!
//! - [`permission_denied`] builds the canonical non-leaking `PERMISSION_DENIED` —
//!   message *"Permission '{p}' denied on resource '{r}' (or it might not
//!   exist)."* — so an unauthorized caller learns neither that the **Permission**
//!   was the gate nor whether the **Resource name** exists.
//! - [`not_found_via_parent`] implements the AIP-211 fallback for a resource that
//!   does *not* exist: reveal the non-existence with `NOT_FOUND` only when the
//!   caller is authorized to read the parent's children, and otherwise fall back
//!   to the same non-leaking `PERMISSION_DENIED`.
//!
//! These *shape* the error; they make no authorization **decision** — whether the
//! caller holds the **Permission**, and whether it may read the parent's children,
//! is the caller's to decide (behind the opt-in `eval` adapter, ADR-0010) and is
//! passed in. Both carry an `IAM_*` `ErrorInfo` under the shared `aip-rs` domain,
//! the AIP-211 analog of the AIP-193 details on [`From<Error>`](crate::Error)
//! (ADR-0007). See <https://google.aip.dev/211>.

use std::collections::HashMap;

use tonic_types::{ErrorDetails, StatusExt};

use crate::{Permission, ERROR_DOMAIN};

/// The AIP-193 `reason` for the AIP-211 authorization failure — unique within the
/// [`ERROR_DOMAIN`], like the `IAM_*` reasons the [`Error`](crate::Error) mapping
/// uses.
const PERMISSION_DENIED_REASON: &str = "IAM_PERMISSION_DENIED";

/// The AIP-193 `reason` for the `NOT_FOUND` arm of [`not_found_via_parent`].
const RESOURCE_NOT_FOUND_REASON: &str = "IAM_RESOURCE_NOT_FOUND";

/// Build the canonical AIP-211 `PERMISSION_DENIED` for an unauthorized request:
/// `permission` denied on `resource`, with the **non-leaking** message that hides
/// whether the resource exists.
///
/// The message is exactly *"Permission '{permission}' denied on resource
/// '{resource}' (or it might not exist)."* — the "(or it might not exist)" tail is
/// what keeps a caller who lacks the **Permission** from distinguishing a denial
/// from a missing resource. An `IAM_*` `ErrorInfo` carries the machine-readable
/// reason + [`domain`](ERROR_DOMAIN) and mirrors the message's `permission` /
/// `resource` values as `metadata` (AIP-193), so a machine actor never parses the
/// prose.
pub fn permission_denied(permission: &Permission, resource: &str) -> tonic::Status {
    let message = format!(
        "Permission '{permission}' denied on resource '{resource}' (or it might not exist)."
    );
    let metadata = HashMap::from([
        ("permission".to_owned(), permission.to_string()),
        ("resource".to_owned(), resource.to_owned()),
    ]);
    let mut details = ErrorDetails::new();
    details.set_error_info(PERMISSION_DENIED_REASON, ERROR_DOMAIN, metadata);
    tonic::Status::with_error_details(tonic::Code::PermissionDenied, message, details)
}

/// The AIP-211 fallback for a **Resource name** that does not exist: shape the
/// error *without leaking existence to a caller who has no business knowing*.
///
/// When `resource` is missing the service cannot determine authorization from the
/// resource's own **Policy**, so AIP-211 says to fall back to the parent: if the
/// caller is authorized to read the parent's children (`parent_read_allowed`),
/// reveal the non-existence with `NOT_FOUND`; otherwise return the same non-leaking
/// [`permission_denied`] an *existing* but unauthorized resource would — so the
/// missing and the forbidden cases are indistinguishable to that caller.
///
/// `parent_read_allowed` is the caller's authorization **decision** for reading the
/// parent's children — this helper only shapes its outcome (ADR-0010). The
/// `NOT_FOUND` arm carries its own `IAM_*` `ErrorInfo` (AIP-193); the denied arm is
/// [`permission_denied`] verbatim.
pub fn not_found_via_parent(
    permission: &Permission,
    resource: &str,
    parent_read_allowed: bool,
) -> tonic::Status {
    if parent_read_allowed {
        let message = format!("Resource '{resource}' not found.");
        let metadata = HashMap::from([("resource".to_owned(), resource.to_owned())]);
        let mut details = ErrorDetails::new();
        details.set_error_info(RESOURCE_NOT_FOUND_REASON, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::NotFound, message, details)
    } else {
        permission_denied(permission, resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission() -> Permission {
        "freight.shippers.get".parse().expect("well-formed")
    }

    #[test]
    fn permission_denied_is_canonical_and_non_leaking() {
        let status = permission_denied(&permission(), "shippers/acme");
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            status.message(),
            "Permission 'freight.shippers.get' denied on resource 'shippers/acme' \
             (or it might not exist).",
        );

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "IAM_PERMISSION_DENIED");
        assert_eq!(info.domain, ERROR_DOMAIN);
        // Every dynamic value in the message is mirrored in metadata (AIP-193).
        assert_eq!(
            info.metadata.get("permission").map(String::as_str),
            Some("freight.shippers.get"),
        );
        assert_eq!(
            info.metadata.get("resource").map(String::as_str),
            Some("shippers/acme"),
        );

        // A permission/resource is an opaque value, not a named request field.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn not_found_via_parent_reveals_only_when_parent_is_readable() {
        // Parent read-children check passes: the caller is allowed to know the
        // resource is missing, so reveal it with NOT_FOUND.
        let revealed = not_found_via_parent(&permission(), "shippers/ghost", true);
        assert_eq!(revealed.code(), tonic::Code::NotFound);
        assert_eq!(revealed.message(), "Resource 'shippers/ghost' not found.");
        let info = revealed
            .get_details_error_info()
            .expect("an ErrorInfo is attached (AIP-193)");
        assert_eq!(info.reason, "IAM_RESOURCE_NOT_FOUND");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("resource").map(String::as_str),
            Some("shippers/ghost"),
        );
    }

    #[test]
    fn not_found_via_parent_hides_existence_when_parent_is_not_readable() {
        // Parent read-children check fails: a missing resource is indistinguishable
        // from a forbidden one — the same non-leaking PERMISSION_DENIED.
        let hidden = not_found_via_parent(&permission(), "shippers/ghost", false);
        let denied = permission_denied(&permission(), "shippers/ghost");
        assert_eq!(hidden.code(), tonic::Code::PermissionDenied);
        assert_eq!(hidden.message(), denied.message());
        assert_eq!(
            hidden
                .get_details_error_info()
                .expect("an ErrorInfo is attached")
                .reason,
            "IAM_PERMISSION_DENIED",
        );
    }
}
