//! Reflection over AIP **resource annotations** — the `google.api.resource` and
//! `google.api.resource_reference` options carried on protobuf descriptors.
//!
//! This is distinct from the `prost-reflect` *mechanics* (descriptor pools,
//! dynamic messages) it is built on: this crate reads the AIP annotations off a
//! [`Descriptor`](prost_reflect) and validates references between resources.
//! Exposed primitives:
//!
//! - [`ResourceType`] — parse a resource type string such as
//!   `freight-example.einride.tech/Shipper` into its service name and type, and
//!   validate it.
//! - [`resource_descriptors_in_file`] / [`resource_descriptors_in_package`] —
//!   enumerate the [`ResourceDescriptor`]s declared in a file or package via the
//!   `google.api.resource` / `google.api.resource_definition` extensions.
//! - [`request_descriptors_in_file`] — digest each top-level message's
//!   AIP-standard request fields (`page_token`, `order_by`, …) into a
//!   [`RequestDescriptor`], driving the codegen plugin's request-trait
//!   emission (this one has no aip-go counterpart — Go satisfies these
//!   interfaces structurally).
//! - [`validate_resource_references`] (and its Dynamic core
//!   [`validate_resource_references_dynamic`]) — for every field carrying a
//!   `google.api.resource_reference`, check the value is a valid name of the
//!   referenced resource type.
//!
//! Like the other reflective primitives this is expressed in the **Typed facade /
//! Dynamic core** shape: the headline [`validate_resource_references`] takes a
//! concrete [`ReflectMessage`](prost_reflect::ReflectMessage) (`prost_reflect`) type, layered over a public
//! [`validate_resource_references_dynamic`] that works on a [`DynamicMessage`](prost_reflect::DynamicMessage)
//! directly — the escape hatch and the crates' test surface.
//!
//! Validation failures return a typed [`Error`] that maps, behind the `tonic`
//! feature, to `INVALID_ARGUMENT` with AIP-193 standard error details.
//!
//! See <https://google.aip.dev/123> (resource types) and
//! <https://google.aip.dev/124> (resource references).
//!
//! # Example
//!
//! ```
//! use aip_reflect::{resource_descriptors_in_package, ResourceType};
//!
//! // parse a resource type into service name + type
//! let ty = ResourceType::new("freight-example.einride.tech/Shipper");
//! ty.validate().unwrap();
//! assert_eq!(ty.service_name(), "freight-example.einride.tech");
//! assert_eq!(ty.type_name(), "Shipper");
//!
//! // enumerate the `google.api.resource` descriptors of a package
//! let pool = test_fixtures::pool();
//! let resources = resource_descriptors_in_package(&pool, "einride.example.freight.v1");
//! assert!(resources
//!     .iter()
//!     .any(|r| r.resource_type.as_str() == "freight-example.einride.tech/Shipper"));
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

mod requests;
mod resource_type;
mod resources;
mod validate;

pub use requests::{request_descriptors_in_file, RequestDescriptor};
pub use resource_type::ResourceType;
pub use resources::{
    resource_descriptors_in_file, resource_descriptors_in_package, ResourceDescriptor,
};
pub use validate::{validate_resource_references, validate_resource_references_dynamic};

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A resource type string is syntactically invalid (see
    /// [`ResourceType::validate`]). `reason` carries the specific failure, e.g.
    /// `"service name: must be a valid domain name"`.
    #[error("invalid resource type `{resource_type}`: {reason}")]
    InvalidResourceType {
        resource_type: String,
        reason: String,
    },

    /// A `google.api.resource_reference` field's value is not a valid name of
    /// the referenced resource type — it matches none of that resource's
    /// declared patterns. `field` is the dotted/indexed path to the offending
    /// field (e.g. `origin_site`, `shipment.origin_site`, `names[1]`).
    #[error(
        "value `{value}` of field {field} is not a valid resource reference for {resource_type}"
    )]
    InvalidResourceReference {
        field: String,
        value: String,
        resource_type: String,
    },
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// (machine-readable `reason` + `domain` (`aip-rs`), with the error's
    /// dynamic values as `metadata`) and, when the error names a request field
    /// path, a `BadRequest` field violation keyed on that path.
    /// See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata, violation): (
            &str,
            HashMap<String, String>,
            Option<(String, String)>,
        ) = match &err {
            Error::InvalidResourceType {
                resource_type,
                reason,
            } => (
                "RESOURCE_TYPE_INVALID",
                HashMap::from([
                    ("resource_type".to_owned(), resource_type.clone()),
                    ("detail".to_owned(), reason.clone()),
                ]),
                None,
            ),
            Error::InvalidResourceReference {
                field,
                value,
                resource_type,
            } => (
                "RESOURCE_REFERENCE_INVALID",
                HashMap::from([
                    ("field".to_owned(), field.clone()),
                    ("value".to_owned(), value.clone()),
                    ("type".to_owned(), resource_type.clone()),
                ]),
                Some((
                    field.clone(),
                    format!("not a valid resource reference for {resource_type}"),
                )),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        if let Some((field, description)) = violation {
            details.add_bad_request_violation(field, description);
        }
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn invalid_resource_type_has_error_info_but_no_bad_request() {
        let status: tonic::Status = Error::InvalidResourceType {
            resource_type: "pubsub/Topic".to_owned(),
            reason: "service name: must be a valid domain name".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "RESOURCE_TYPE_INVALID");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn invalid_resource_reference_attaches_bad_request_violation() {
        let status: tonic::Status = Error::InvalidResourceReference {
            field: "origin_site".to_owned(),
            value: "shippers/1".to_owned(),
            resource_type: "freight-example.einride.tech/Site".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "RESOURCE_REFERENCE_INVALID");
        assert_eq!(info.domain, ERROR_DOMAIN);

        let bad = status
            .get_details_bad_request()
            .expect("BadRequest attached for field-path errors");
        assert_eq!(bad.field_violations.len(), 1);
        assert_eq!(bad.field_violations[0].field, "origin_site");
    }
}
