//! AIP-161 field masks: apply update masks and validate paths.
//!
//! The mask type is `google.protobuf.FieldMask` (`prost_types::FieldMask`) —
//! exactly how it arrives in an update request. The mask itself is plain data;
//! only the *message* side is reflective.
//!
//! Applying an update mask is presented as a **Typed facade** ([`update`],
//! generic over [`ReflectMessage`], so a message carries its own descriptor)
//! layered on a still-public **Dynamic core** ([`update_dynamic`], over
//! [`DynamicMessage`]).
//!
//! The core is a port of `go.einride.tech/aip/fieldmask`. It departs from the
//! reference in one place: a source/destination type mismatch returns
//! [`Error::TypeMismatch`] where aip-go panics.
//!
//! See <https://google.aip.dev/161> and <https://google.aip.dev/134>.
//!
//! # Example
//!
//! ```
//! use aip_fieldmask::update_dynamic;
//! use prost_types::FieldMask;
//!
//! let mut dst = test_fixtures::from_json(
//!     "einride.example.freight.v1.Shipper",
//!     r#"{"name": "shippers/acme", "displayName": "Acme"}"#,
//! )
//! .unwrap();
//! let src = test_fixtures::from_json(
//!     "einride.example.freight.v1.Shipper",
//!     r#"{"displayName": "Acme Freight"}"#,
//! )
//! .unwrap();
//!
//! let mask = FieldMask { paths: vec!["display_name".into()] };
//! update_dynamic(&mask, &mut dst, &src).unwrap();
//!
//! // masked field updated, unmasked field untouched
//! let display_name = dst.get_field_by_name("display_name").unwrap();
//! assert_eq!(display_name.as_str(), Some("Acme Freight"));
//! let name = dst.get_field_by_name("name").unwrap();
//! assert_eq!(name.as_str(), Some("shippers/acme"));
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

use prost::Message as _;
use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage};
use prost_types::FieldMask;

/// The single path that marks an [update mask](self) as a [`Full replacement`].
///
/// [`Full replacement`]: is_full_replacement
const WILDCARD_PATH: &str = "*";

/// Errors produced when applying or validating a field mask.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A path names a field that does not exist on the message it is applied to.
    #[error("field mask path {path:?} does not exist on message {message}")]
    UnknownPath { path: String, message: String },
    /// The full-replacement path `*` was combined with other paths.
    #[error("field mask path \"*\" must not be combined with other paths")]
    WildcardNotAlone,
    /// [`update_dynamic`] was given a destination and source of different
    /// message types. (The typed [`update`] facade fixes both to one type, so it
    /// cannot raise this.)
    #[error("dst and src message types differ: {dst} vs {src}")]
    TypeMismatch { dst: String, src: String },
}

/// Reports whether `mask` is the full-replacement mask (`["*"]`).
///
/// The full-replacement mask means "replace every field" — the equivalent of an
/// HTTP `PUT` — rather than a selective update.
pub fn is_full_replacement(mask: &FieldMask) -> bool {
    mask.paths.len() == 1 && mask.paths[0] == WILDCARD_PATH
}

/// The [Dynamic core](self) of the update-mask primitive: applies an AIP-134
/// update mask, copying masked fields from `src` into `dst` (ADR-0009).
///
/// This is the low-level interface over [`DynamicMessage`] — the escape hatch
/// for a caller who already holds a dynamic message (JSON ingestion, a generic
/// gateway) and the crate's test surface. Callers holding concrete generated
/// types reach for the [`update`] **Typed facade**, which transcodes onto this.
///
/// - An **empty** mask copies only the fields populated on `src` (proto3
///   presence: a default-valued scalar without explicit presence counts as
///   unpopulated), recursing into singular message fields.
/// - `["*"]` is a [`Full replacement`]: `dst` becomes a copy of `src`.
/// - Otherwise each path is copied, and a masked path absent from `src` clears
///   that field in `dst`.
///
/// Paths that descend into a map or repeated field, or that name an unknown
/// field, are ignored — matching aip-go. Returns [`Error::TypeMismatch`] if the
/// descriptors differ (where aip-go panics).
///
/// [`Full replacement`]: is_full_replacement
pub fn update_dynamic(
    mask: &FieldMask,
    dst: &mut DynamicMessage,
    src: &DynamicMessage,
) -> Result<(), Error> {
    let dst_desc = dst.descriptor();
    let src_desc = src.descriptor();
    if dst_desc != src_desc {
        return Err(Error::TypeMismatch {
            dst: dst_desc.full_name().to_owned(),
            src: src_desc.full_name().to_owned(),
        });
    }

    if mask.paths.is_empty() {
        // No update mask: copy every field set on the wire in `src`.
        update_wire_set_fields(dst, src);
    } else if is_full_replacement(mask) {
        // `["*"]`: a full replacement of all fields (reset + merge).
        *dst = src.clone();
    } else {
        for path in &mask.paths {
            let segments: Vec<&str> = path.split('.').collect();
            update_named_field(dst, src, &segments);
        }
    }
    Ok(())
}

/// Applies an AIP-134 update mask to a **Typed message**, copying masked fields
/// from `src` into `dst`. The headline interface over the [`update_dynamic`]
/// core (ADR-0009).
///
/// `dst` and `src` are concrete generated messages that carry their own
/// [`Descriptor`](ReflectMessage::descriptor); the facade transcodes both to
/// [`DynamicMessage`]s through their wire bytes, runs the core, and decodes the
/// result back into `dst`. Same masking semantics as [`update_dynamic`]: an
/// empty mask copies `src`'s populated fields, `["*"]` is a [`Full replacement`],
/// and a named path absent from `src` clears that field.
///
/// The `M → DynamicMessage → M` round-trip can fail only if a type and its
/// descriptor disagree — a build/config bug, not bad input — so it is treated as
/// an invariant (`expect`), not an [`Error`] variant (ADR-0009). Because `dst`
/// and `src` share the type `M`, the core's [`Error::TypeMismatch`] is
/// unreachable here; the `Result` mirrors the core for a uniform call surface.
///
/// [`Full replacement`]: is_full_replacement
pub fn update<M: ReflectMessage + Default>(
    mask: &FieldMask,
    dst: &mut M,
    src: &M,
) -> Result<(), Error> {
    let descriptor = dst.descriptor();
    let mut dst_dynamic =
        DynamicMessage::decode(descriptor.clone(), dst.encode_to_vec().as_slice())
            .expect("a message round-trips through its own descriptor");
    let src_dynamic = DynamicMessage::decode(descriptor, src.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    update_dynamic(mask, &mut dst_dynamic, &src_dynamic)?;
    *dst = M::decode(dst_dynamic.encode_to_vec().as_slice())
        .expect("a dynamic message re-decodes into its generated type");
    Ok(())
}

/// Copies every field populated on `src` into `dst`, recursing into singular
/// message fields so a set nested field merges rather than replaces.
///
/// Mirrors aip-go's `updateWireSetFields`: lists and maps are copied wholesale,
/// an unset singular message is copied, and a set singular message is merged.
fn update_wire_set_fields(dst: &mut DynamicMessage, src: &DynamicMessage) {
    for (field, value) in src.fields() {
        let is_singular_message =
            matches!(field.kind(), Kind::Message(_)) && !field.is_list() && !field.is_map();
        if is_singular_message && dst.has_field(&field) {
            let src_msg = value
                .as_message()
                .expect("a message-kind field holds a message value");
            let dst_msg = dst
                .get_field_mut(&field)
                .as_message_mut()
                .expect("a message-kind field holds a message value");
            update_wire_set_fields(dst_msg, src_msg);
        } else {
            dst.set_field(&field, value.clone());
        }
    }
}

/// Applies a single dotted path, descending one segment per recursion.
///
/// Mirrors aip-go's `updateNamedField`: a leaf path sets the field from `src`,
/// or clears it in `dst` when `src` does not have it; an interior path recurses
/// into a singular message field, allocating it on `dst` if absent. Unknown
/// fields and paths descending into a map or repeated field are ignored.
fn update_named_field(dst: &mut DynamicMessage, src: &DynamicMessage, segments: &[&str]) {
    let Some((first, rest)) = segments.split_first() else {
        return;
    };
    let Some(field) = src.descriptor().get_field_by_name(first) else {
        return; // no known field by that name
    };

    // A named field in this message.
    if rest.is_empty() {
        if src.has_field(&field) {
            dst.set_field(&field, src.get_field(&field).into_owned());
        } else {
            dst.clear_field(&field);
        }
        return;
    }

    // A named field in a nested message.
    if field.is_list() || field.is_map() {
        return; // nested fields in a repeated field or map are not supported
    }
    if matches!(field.kind(), Kind::Message(_)) {
        // Read `src`'s submessage (an empty default if it is unset), then merge
        // into `dst`'s, allocating an empty submessage on `dst` if absent.
        let src_msg = src
            .get_field(&field)
            .as_message()
            .expect("a message-kind field holds a message value")
            .clone();
        let dst_msg = dst
            .get_field_mut(&field)
            .as_message_mut()
            .expect("a message-kind field holds a message value");
        update_named_field(dst_msg, &src_msg, rest);
    }
}

/// Validates that every path in `mask` resolves to a field on `descriptor`.
///
/// Takes a descriptor, not a message instance — you need no value to check
/// paths. A path may descend through singular, repeated, or map-valued message
/// fields with `.`. The full-replacement path `*` is valid only on its own;
/// combined with any other path it is [`Error::WildcardNotAlone`].
pub fn validate(mask: &FieldMask, descriptor: &MessageDescriptor) -> Result<(), Error> {
    if mask.paths.iter().any(|path| path == WILDCARD_PATH) {
        if mask.paths.len() != 1 {
            return Err(Error::WildcardNotAlone);
        }
        return Ok(());
    }

    for path in &mask.paths {
        if !path_exists(path, descriptor) {
            return Err(Error::UnknownPath {
                path: path.clone(),
                message: descriptor.full_name().to_owned(),
            });
        }
    }
    Ok(())
}

/// Reports whether a dotted `path` resolves field-by-field through `descriptor`,
/// descending into the message type each non-leaf segment names (the map value's
/// type for a map field).
fn path_exists(path: &str, descriptor: &MessageDescriptor) -> bool {
    let mut current = Some(descriptor.clone());
    for segment in path.split('.') {
        let Some(message) = current else {
            return false; // not within a message
        };
        let Some(field) = message.get_field_by_name(segment) else {
            return false; // message does not have this field
        };
        // Identify the next message to search within (may be a scalar -> None).
        current = next_message(&field);
    }
    true
}

/// The message type to descend into through `field`: its value type for a map,
/// otherwise the field's own message type. `None` for a scalar or enum leaf.
fn next_message(field: &FieldDescriptor) -> Option<MessageDescriptor> {
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            return None;
        };
        match entry.map_entry_value_field().kind() {
            Kind::Message(message) => Some(message),
            _ => None,
        }
    } else {
        match field.kind() {
            Kind::Message(message) => Some(message),
            _ => None,
        }
    }
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// (machine-readable `reason` + `domain` (`aip-rs`), with the error's
    /// dynamic values as `metadata`) and, when the error names a mask path, a
    /// `BadRequest` field violation keyed on that path.
    /// See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        // The optional field violation carries the offending mask path — a path
        // leading to a field, exactly what AIP-193's `BadRequest` expects.
        let (reason, metadata, violation): (
            &str,
            HashMap<String, String>,
            Option<(String, String)>,
        ) = match &err {
            Error::UnknownPath { path, message } => (
                "FIELD_MASK_UNKNOWN_PATH",
                HashMap::from([
                    ("path".to_owned(), path.clone()),
                    ("message".to_owned(), message.clone()),
                ]),
                Some((path.clone(), format!("does not exist on message {message}"))),
            ),
            Error::WildcardNotAlone => ("FIELD_MASK_WILDCARD_NOT_ALONE", HashMap::new(), None),
            Error::TypeMismatch { dst, src } => (
                "FIELD_MASK_TYPE_MISMATCH",
                HashMap::from([
                    ("dst".to_owned(), dst.clone()),
                    ("src".to_owned(), src.clone()),
                ]),
                None,
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
    fn unknown_path_attaches_bad_request_field_violation() {
        let status: tonic::Status = Error::UnknownPath {
            path: "no_such_field".to_owned(),
            message: "test.Msg".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "FIELD_MASK_UNKNOWN_PATH");
        assert_eq!(info.domain, ERROR_DOMAIN);

        // The offending mask path is a field path, so it surfaces as a violation.
        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest is attached for path errors");
        assert_eq!(bad.field_violations.len(), 1);
        assert_eq!(bad.field_violations[0].field, "no_such_field");
    }

    #[test]
    fn wildcard_not_alone_has_error_info_but_no_bad_request() {
        let status: tonic::Status = Error::WildcardNotAlone.into();
        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "FIELD_MASK_WILDCARD_NOT_ALONE");
        // No single field path is at fault, so there is no BadRequest.
        assert!(status.get_details_bad_request().is_none());
    }
}
