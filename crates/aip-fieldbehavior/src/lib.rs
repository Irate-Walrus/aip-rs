//! AIP-161/AIP-203 field behavior: read, clear, copy, and validate fields by
//! their `google.api.field_behavior` annotation.
//!
//! This is a **Reflective primitive** — it needs a message's **Descriptor** to
//! read the `google.api.field_behavior` extension off a field. It is expressed
//! in the **Typed facade / Dynamic core** shape:
//!
//! - **Typed facades** (`clear_fields`, `copy_fields`, `validate_required`,
//!   `validate_required_with_mask`, `validate_immutable_not_changed`) operate on
//!   concrete [`ReflectMessage`] types; the caller never builds a
//!   [`DynamicMessage`].
//! - **Dynamic cores** (`clear_fields_dynamic`, `copy_fields_dynamic`,
//!   `validate_required_dynamic`, `validate_required_with_mask_dynamic`,
//!   `validate_immutable_not_changed_dynamic`) operate on [`DynamicMessage`]
//!   directly — the test surface and escape hatch for callers with a dynamic
//!   message.
//!
//! The transform side (`clear_fields`, `copy_fields`) is **infallible**; a
//! type mismatch in `copy_fields_dynamic` is a programmer error and panics. The
//! validation side is **fallible** and returns a typed [`Error`] that maps,
//! behind the `tonic` feature, to `INVALID_ARGUMENT` with AIP-193 standard
//! error details.
//!
//! See <https://google.aip.dev/161> and <https://google.aip.dev/203>.
//!
//! # Example
//!
//! ```
//! use aip_fieldbehavior::{clear_fields_dynamic, has, FieldBehavior};
//!
//! let desc = test_fixtures::message_descriptor("einride.example.freight.v1.Shipper").unwrap();
//! let create_time = desc.get_field_by_name("create_time").unwrap();
//! assert!(has(&create_time, FieldBehavior::OutputOnly));
//!
//! // strip client-supplied OUTPUT_ONLY fields before storing
//! let mut shipper = test_fixtures::from_json(
//!     "einride.example.freight.v1.Shipper",
//!     r#"{"name": "shippers/acme", "createTime": "2026-01-01T00:00:00Z"}"#,
//! )
//! .unwrap();
//! clear_fields_dynamic(&mut shipper, &[FieldBehavior::OutputOnly]);
//! assert!(!shipper.has_field_by_name("create_time"));
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

use prost::Message as _;
use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, ReflectMessage, Value};
use prost_types::FieldMask;

/// The field number of the `google.api.field_behavior` extension on
/// `google.protobuf.FieldOptions`.
const FIELD_BEHAVIOR_TAG: u32 = 1052;

/// The behavior annotation values from `google.api.FieldBehavior`.
///
/// Mirrors the proto enum by the same name. Values that appear in the proto but
/// are not yet used by this crate are included for completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FieldBehavior {
    Optional = 1,
    Required = 2,
    OutputOnly = 3,
    InputOnly = 4,
    Immutable = 5,
    UnorderedList = 6,
    NonEmptyDefault = 7,
    Identifier = 8,
}

impl TryFrom<i32> for FieldBehavior {
    type Error = i32;

    fn try_from(n: i32) -> Result<Self, Self::Error> {
        match n {
            1 => Ok(Self::Optional),
            2 => Ok(Self::Required),
            3 => Ok(Self::OutputOnly),
            4 => Ok(Self::InputOnly),
            5 => Ok(Self::Immutable),
            6 => Ok(Self::UnorderedList),
            7 => Ok(Self::NonEmptyDefault),
            8 => Ok(Self::Identifier),
            other => Err(other),
        }
    }
}

/// Returns the `google.api.field_behavior` annotations on `field`, or an empty
/// vec if the extension is absent or the pool does not have `field_behavior.proto`.
pub fn get(field: &FieldDescriptor) -> Vec<FieldBehavior> {
    let pool = field.parent_pool();
    let options = field.options();
    let Some(field_options_desc) = pool.get_message_by_name("google.protobuf.FieldOptions") else {
        return vec![];
    };
    let Some(ext_desc) = field_options_desc.get_extension(FIELD_BEHAVIOR_TAG) else {
        return vec![];
    };
    if !options.has_extension(&ext_desc) {
        return vec![];
    }
    let value = options.get_extension(&ext_desc);
    let Value::List(list) = value.as_ref() else {
        return vec![];
    };
    list.iter()
        .filter_map(|v| {
            if let Value::EnumNumber(n) = v {
                FieldBehavior::try_from(*n).ok()
            } else {
                None
            }
        })
        .collect()
}

/// Returns `true` if `field` carries the `want` behavior annotation.
pub fn has(field: &FieldDescriptor, want: FieldBehavior) -> bool {
    get(field).contains(&want)
}

// ── Transform side (infallible) ──────────────────────────────────────────────

/// The [Dynamic core](self) of the clear-by-behavior primitive: clears every
/// field in `msg` (recursing into nested messages, lists, and maps) that carries
/// any of the given `behaviors` (ADR-0009).
///
/// This is infallible — clearing a field is always safe. Use this when you
/// already hold a [`DynamicMessage`]; callers with a concrete generated type
/// reach for the [`clear_fields`] **Typed facade**.
pub fn clear_fields_dynamic(msg: &mut DynamicMessage, behaviors: &[FieldBehavior]) {
    clear_in_dynamic(msg, behaviors);
}

/// Clears every field in `msg` that carries any of the given `behaviors`,
/// recursing into nested messages, lists, and maps. The headline **Typed
/// facade** over [`clear_fields_dynamic`] (ADR-0009).
///
/// Use this to ignore client-supplied `OUTPUT_ONLY` or `IMMUTABLE` fields
/// arriving in a create/update request (AIP-161). The `M → DynamicMessage → M`
/// transcode round-trip is infallible by construction (ADR-0009).
pub fn clear_fields<M: ReflectMessage + Default>(msg: &mut M, behaviors: &[FieldBehavior]) {
    let descriptor = msg.descriptor();
    let mut dynamic = DynamicMessage::decode(descriptor, msg.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    clear_fields_dynamic(&mut dynamic, behaviors);
    *msg = M::decode(dynamic.encode_to_vec().as_slice())
        .expect("a dynamic message re-decodes into its generated type");
}

/// The [Dynamic core](self) of the copy-by-behavior primitive: copies every
/// field carrying any of the given `behaviors` from `src` into `dst` (ADR-0009).
///
/// If the source field is "present" (non-zero for scalars, non-nil for
/// messages, non-empty for lists/maps) it is set on `dst`; otherwise the
/// corresponding `dst` field is cleared. Panics if `dst` and `src` have
/// different message types (a programmer bug — use the [`copy_fields`] **Typed
/// facade** to rule this out at compile time).
pub fn copy_fields_dynamic(
    dst: &mut DynamicMessage,
    src: &DynamicMessage,
    behaviors: &[FieldBehavior],
) {
    if dst.descriptor() != src.descriptor() {
        panic!(
            "different types of dst ({}) and src ({})",
            dst.descriptor().full_name(),
            src.descriptor().full_name(),
        );
    }
    let descriptor = dst.descriptor().clone();
    for field in descriptor.fields() {
        let field_behaviors = get(&field);
        if !has_any_behavior(&field_behaviors, behaviors) {
            continue;
        }
        if is_field_present(src, &field) {
            dst.set_field(&field, src.get_field(&field).into_owned());
        } else {
            dst.clear_field(&field);
        }
    }
}

/// Copies every field carrying any of the given `behaviors` from `src` into
/// `dst`. The headline **Typed facade** over [`copy_fields_dynamic`] (ADR-0009).
///
/// Use this to restore server-owned `OUTPUT_ONLY` fields (e.g. `create_time`,
/// `delete_time`) from the stored resource into the updated copy. The typed
/// facade fixes both `dst` and `src` to the same type `M`, so the
/// `copy_fields_dynamic` type-mismatch panic is unreachable here.
pub fn copy_fields<M: ReflectMessage + Default>(dst: &mut M, src: &M, behaviors: &[FieldBehavior]) {
    let descriptor = dst.descriptor();
    let mut dst_dynamic =
        DynamicMessage::decode(descriptor.clone(), dst.encode_to_vec().as_slice())
            .expect("a message round-trips through its own descriptor");
    let src_dynamic = DynamicMessage::decode(descriptor, src.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    copy_fields_dynamic(&mut dst_dynamic, &src_dynamic, behaviors);
    *dst = M::decode(dst_dynamic.encode_to_vec().as_slice())
        .expect("a dynamic message re-decodes into its generated type");
}

// ── Validation side (fallible) ────────────────────────────────────────────────

/// Errors produced by the validation side of the field-behavior primitive.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// One or more fields tagged `REQUIRED` are unset (zero / nil / empty) in the
    /// message. The validator accumulates **every** missing path rather than
    /// bailing on the first, so a caller sees all violations in one response
    /// (ADR-0007).
    #[error("missing required fields: {}", paths.join(", "))]
    RequiredFields { paths: Vec<String> },
    /// A field tagged `IMMUTABLE` changed its value within the update mask.
    #[error("immutable field cannot be changed: {path}")]
    ImmutableField { path: String },
    /// The `old` and `updated` messages passed to
    /// [`validate_immutable_not_changed_dynamic`] have different types.
    #[error("old and updated messages have different types: {old} vs {updated}")]
    TypeMismatch { old: String, updated: String },
}

/// The [Dynamic core](self) of the required-field validator: returns
/// [`Error::RequiredFields`] listing **every** field tagged `REQUIRED` that is
/// unset, recursing into nested messages, lists, and maps (ADR-0009).
///
/// Equivalent to calling [`validate_required_with_mask_dynamic`] with a
/// wildcard mask — all `REQUIRED` fields are checked.
pub fn validate_required_dynamic(msg: &DynamicMessage) -> Result<(), Error> {
    let mut paths = Vec::new();
    validate_required_inner(msg, None, "", &mut paths);
    into_required_result(paths)
}

/// Returns [`Error::RequiredFields`] listing every `REQUIRED` field in `msg`
/// that is unset. The headline **Typed facade** over
/// [`validate_required_dynamic`] (ADR-0009).
pub fn validate_required<M: ReflectMessage>(msg: &M) -> Result<(), Error> {
    let descriptor = msg.descriptor();
    let dynamic = DynamicMessage::decode(descriptor, msg.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    validate_required_dynamic(&dynamic)
}

/// The [Dynamic core](self) of the masked required-field validator: validates
/// only `REQUIRED` fields whose **exact** path appears in `mask` (ADR-0009).
///
/// An empty mask is a no-op — the mask is treated as "only fields explicitly
/// on the wire need be checked", and an empty mask means nothing was sent.
/// Recursion into nested messages, lists, and maps follows the same path semantics.
pub fn validate_required_with_mask_dynamic(
    msg: &DynamicMessage,
    mask: &FieldMask,
) -> Result<(), Error> {
    if mask.paths.is_empty() {
        return Ok(());
    }
    let mut paths = Vec::new();
    validate_required_inner(msg, Some(mask), "", &mut paths);
    into_required_result(paths)
}

/// Validates only `REQUIRED` fields whose exact path appears in `mask`. The
/// headline **Typed facade** over [`validate_required_with_mask_dynamic`]
/// (ADR-0009).
///
/// An empty mask is a no-op. Use this on update requests to validate only the
/// fields the client intends to change.
pub fn validate_required_with_mask<M: ReflectMessage>(
    msg: &M,
    mask: &FieldMask,
) -> Result<(), Error> {
    let descriptor = msg.descriptor();
    let dynamic = DynamicMessage::decode(descriptor, msg.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    validate_required_with_mask_dynamic(&dynamic, mask)
}

/// The [Dynamic core](self) of the immutable-field change validator: returns
/// [`Error::ImmutableField`] if any field tagged `IMMUTABLE` within `mask`
/// actually changed between `old` and `updated` (ADR-0009).
///
/// Compliant with AIP-203: a client may include an `IMMUTABLE` field in the
/// update mask as long as it is set to its existing value — only a genuine
/// value change is an error. Returns [`Error::TypeMismatch`] if `old` and
/// `updated` have different message types.
///
/// The deprecated `ValidateImmutableFieldsWithMask` variant from aip-go (which
/// errors on mere presence in the mask) is intentionally not ported.
pub fn validate_immutable_not_changed_dynamic(
    old: &DynamicMessage,
    updated: &DynamicMessage,
    mask: &FieldMask,
) -> Result<(), Error> {
    if old.descriptor() != updated.descriptor() {
        return Err(Error::TypeMismatch {
            old: old.descriptor().full_name().to_owned(),
            updated: updated.descriptor().full_name().to_owned(),
        });
    }
    validate_immutable_inner(old, updated, mask, "")
}

/// Returns [`Error::ImmutableField`] if any `IMMUTABLE` field within `mask`
/// changed between `old` and `updated`. The headline **Typed facade** over
/// [`validate_immutable_not_changed_dynamic`] (ADR-0009).
///
/// Because `old` and `updated` share the type `M`, [`Error::TypeMismatch`] is
/// unreachable through this facade; it is propagated through `?` for a uniform
/// call surface with the dynamic core.
pub fn validate_immutable_not_changed<M: ReflectMessage>(
    old: &M,
    updated: &M,
    mask: &FieldMask,
) -> Result<(), Error> {
    let descriptor = old.descriptor();
    let old_dynamic = DynamicMessage::decode(descriptor.clone(), old.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    let updated_dynamic = DynamicMessage::decode(descriptor, updated.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    validate_immutable_not_changed_dynamic(&old_dynamic, &updated_dynamic, mask)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn has_any_behavior(haystack: &[FieldBehavior], needles: &[FieldBehavior]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Whether a field value is "present": non-nil for messages, non-empty for
/// lists/maps, and non-zero for scalars. Mirrors Go's `isMessageFieldPresent`.
fn is_field_present(msg: &DynamicMessage, field: &FieldDescriptor) -> bool {
    if field.is_list() {
        let v = msg.get_field(field);
        return matches!(v.as_ref(), Value::List(l) if !l.is_empty());
    }
    if field.is_map() {
        let v = msg.get_field(field);
        return matches!(v.as_ref(), Value::Map(m) if !m.is_empty());
    }
    match field.kind() {
        Kind::Message(_) => msg.has_field(field),
        Kind::Bool => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::Bool(true))
        }
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::I32(n) if *n != 0)
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::I64(n) if *n != 0)
        }
        Kind::Uint32 | Kind::Fixed32 => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::U32(n) if *n != 0)
        }
        Kind::Uint64 | Kind::Fixed64 => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::U64(n) if *n != 0)
        }
        Kind::Float => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::F32(f) if *f != 0.0)
        }
        Kind::Double => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::F64(f) if *f != 0.0)
        }
        Kind::String => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::String(s) if !s.is_empty())
        }
        Kind::Bytes => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::Bytes(b) if !b.is_empty())
        }
        Kind::Enum(_) => {
            let v = msg.get_field(field);
            matches!(v.as_ref(), Value::EnumNumber(n) if *n != 0)
        }
    }
}

/// Recursively clears fields in `msg` that have any of `behaviors`.
fn clear_in_dynamic(msg: &mut DynamicMessage, behaviors: &[FieldBehavior]) {
    let descriptor = msg.descriptor().clone();
    let all_fields: Vec<FieldDescriptor> = descriptor.fields().collect();

    // Two-pass: collect which fields to clear vs. recurse, then apply.
    // This avoids holding an immutable borrow while we mutate.
    let mut to_clear: Vec<FieldDescriptor> = vec![];
    let mut to_recurse: Vec<FieldDescriptor> = vec![];

    for field in &all_fields {
        if !msg.has_field(field) {
            // For lists/maps, has_field returns false if empty — also skip those.
            if field.is_list() || field.is_map() {
                let v = msg.get_field(field);
                match v.as_ref() {
                    Value::List(l) if l.is_empty() => continue,
                    Value::Map(m) if m.is_empty() => continue,
                    Value::List(_) | Value::Map(_) => {}
                    _ => continue,
                }
            } else {
                continue;
            }
        }
        let field_behaviors = get(field);
        if has_any_behavior(&field_behaviors, behaviors) {
            to_clear.push(field.clone());
        } else if matches!(field.kind(), Kind::Message(_)) {
            to_recurse.push(field.clone());
        }
    }

    for field in &to_clear {
        msg.clear_field(field);
    }

    for field in &to_recurse {
        recurse_clear(msg, field, behaviors);
    }
}

/// Recurses into a set message/list/map field and clears nested behaviors.
fn recurse_clear(msg: &mut DynamicMessage, field: &FieldDescriptor, behaviors: &[FieldBehavior]) {
    if field.is_list() {
        let value_mut = msg.get_field_mut(field);
        if let Value::List(list) = value_mut {
            for item in list.iter_mut() {
                if let Value::Message(nested) = item {
                    clear_in_dynamic(nested, behaviors);
                }
            }
        }
    } else if field.is_map() {
        // Only recurse if map values are messages.
        let Kind::Message(entry_desc) = field.kind() else {
            return;
        };
        if !matches!(entry_desc.map_entry_value_field().kind(), Kind::Message(_)) {
            return;
        }
        let value_mut = msg.get_field_mut(field);
        if let Value::Map(map) = value_mut {
            for item in map.values_mut() {
                if let Value::Message(nested) = item {
                    clear_in_dynamic(nested, behaviors);
                }
            }
        }
    } else {
        let value_mut = msg.get_field_mut(field);
        if let Value::Message(nested) = value_mut {
            clear_in_dynamic(nested, behaviors);
        }
    }
}

/// Collapses an accumulated path list into a result: `Ok(())` when empty, else
/// one [`Error::RequiredFields`] carrying every missing path.
fn into_required_result(paths: Vec<String>) -> Result<(), Error> {
    if paths.is_empty() {
        Ok(())
    } else {
        Err(Error::RequiredFields { paths })
    }
}

/// Inner recursion for `validate_required_dynamic` and `validate_required_with_mask_dynamic`.
///
/// `mask = None` means "check all REQUIRED fields" (the maskless variant).
/// `mask = Some(m)` means "check only fields with an exact path in m".
///
/// Accumulates every missing REQUIRED path into `missing` rather than returning
/// on the first, so the caller reports all violations at once (ADR-0007).
fn validate_required_inner(
    msg: &DynamicMessage,
    mask: Option<&FieldMask>,
    path: &str,
    missing: &mut Vec<String>,
) {
    let descriptor = msg.descriptor();
    for field in descriptor.fields() {
        let curr_path = join_path(path, field.name());

        let present = is_field_present(msg, &field);
        if !present {
            if has(&field, FieldBehavior::Required)
                && mask.is_none_or(|m| has_exact_path(m, &curr_path))
            {
                missing.push(curr_path);
            }
        } else {
            // Recurse into set message fields.
            match field.kind() {
                Kind::Message(_) if field.is_list() => {
                    let v = msg.get_field(&field);
                    if let Value::List(list) = v.as_ref() {
                        for item in list {
                            if let Value::Message(nested) = item {
                                validate_required_inner(nested, mask, &curr_path, missing);
                            }
                        }
                    }
                }
                Kind::Message(_) if field.is_map() => {
                    let Kind::Message(entry_desc) = field.kind() else {
                        continue;
                    };
                    if !matches!(entry_desc.map_entry_value_field().kind(), Kind::Message(_)) {
                        continue;
                    }
                    let v = msg.get_field(&field);
                    if let Value::Map(map) = v.as_ref() {
                        for item in map.values() {
                            if let Value::Message(nested) = item {
                                validate_required_inner(nested, mask, &curr_path, missing);
                            }
                        }
                    }
                }
                Kind::Message(_) => {
                    let v = msg.get_field(&field);
                    if let Value::Message(nested) = v.as_ref() {
                        validate_required_inner(nested, mask, &curr_path, missing);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Inner recursion for `validate_immutable_not_changed_dynamic`.
fn validate_immutable_inner(
    old: &DynamicMessage,
    updated: &DynamicMessage,
    mask: &FieldMask,
    path: &str,
) -> Result<(), Error> {
    let descriptor = old.descriptor();
    for field in descriptor.fields() {
        let curr_path = join_path(path, field.name());

        if has(&field, FieldBehavior::Immutable) && has_path_with_prefix(mask, &curr_path) {
            let old_value = old.get_field(&field);
            let updated_value = updated.get_field(&field);
            if old_value != updated_value {
                return Err(Error::ImmutableField { path: curr_path });
            }
            // Values are equal — skip nested check per Go reference.
            continue;
        }

        match field.kind() {
            Kind::Message(_) if field.is_list() => {
                let old_v = old.get_field(&field);
                let upd_v = updated.get_field(&field);
                let (Value::List(old_list), Value::List(upd_list)) =
                    (old_v.as_ref(), upd_v.as_ref())
                else {
                    continue;
                };
                if old_list.is_empty() || upd_list.is_empty() {
                    continue;
                }
                let min_len = old_list.len().min(upd_list.len());
                for i in 0..min_len {
                    if let (Value::Message(old_msg), Value::Message(upd_msg)) =
                        (&old_list[i], &upd_list[i])
                    {
                        validate_immutable_inner(old_msg, upd_msg, mask, &curr_path)?;
                    }
                }
            }
            Kind::Message(_) if field.is_map() => {
                let Kind::Message(entry_desc) = field.kind() else {
                    continue;
                };
                if !matches!(entry_desc.map_entry_value_field().kind(), Kind::Message(_)) {
                    continue;
                }
                let old_v = old.get_field(&field);
                let upd_v = updated.get_field(&field);
                let (Value::Map(old_map), Value::Map(upd_map)) = (old_v.as_ref(), upd_v.as_ref())
                else {
                    continue;
                };
                if old_map.is_empty() || upd_map.is_empty() {
                    continue;
                }
                for (key, upd_val) in upd_map {
                    if let Some(old_val) = old_map.get(key) {
                        if let (Value::Message(old_msg), Value::Message(upd_msg)) =
                            (old_val, upd_val)
                        {
                            validate_immutable_inner(old_msg, upd_msg, mask, &curr_path)?;
                        }
                    }
                }
            }
            Kind::Message(_) => {
                let old_has = old.has_field(&field);
                let upd_has = updated.has_field(&field);
                if !old_has || !upd_has {
                    continue;
                }
                let old_v = old.get_field(&field);
                let upd_v = updated.get_field(&field);
                if let (Value::Message(old_msg), Value::Message(upd_msg)) =
                    (old_v.as_ref(), upd_v.as_ref())
                {
                    validate_immutable_inner(old_msg, upd_msg, mask, &curr_path)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Joins a path prefix with a field name using `.`, or returns the name when
/// the prefix is empty.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Whether `mask` contains `needle` as an exact path (or `"*"`). Used for
/// `validate_required_with_mask`, which requires exact path matching.
fn has_exact_path(mask: &FieldMask, needle: &str) -> bool {
    mask.paths.iter().any(|p| p == "*" || p == needle)
}

/// Whether `mask` covers `needle` exactly or as a prefix (`"a"` covers
/// `"a.b"`). Used for `validate_immutable_not_changed`, where a parent path
/// in the mask covers nested immutable fields.
fn has_path_with_prefix(mask: &FieldMask, needle: &str) -> bool {
    if mask.paths.is_empty() {
        return true;
    }
    for straw in &mask.paths {
        if straw == "*" || straw == needle {
            return true;
        }
        // Prefix match: "line_items" covers "line_items.external_reference_id".
        if straw.len() < needle.len()
            && needle.starts_with(straw.as_str())
            && needle.as_bytes().get(straw.len()) == Some(&b'.')
        {
            return true;
        }
    }
    false
}

/// The library-internal AIP-193 `ErrorInfo.domain` every error this crate maps
/// is stamped with. It is a sentinel meaning "replace at the serving boundary":
/// a deploying service installs the `aip-errordomain` layer, which rewrites it
/// to the service's own domain so clients see one domain. It is the only
/// conversion — there is no per-call-site re-stamping.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details under the
    /// library sentinel `aip-rs` domain: an `ErrorInfo` on every
    /// error (the AIP-193 MUST) and, when the error names field paths, one
    /// `BadRequest` violation per path. A deploying service rewrites the
    /// sentinel domain to its own at the serving boundary with the
    /// `aip-errordomain` layer. See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata, violations): (&str, HashMap<String, String>, Vec<(String, String)>) =
            match &err {
                Error::RequiredFields { paths } => (
                    "FIELD_REQUIRED",
                    // Mirror the offending paths under `fields`, matching
                    // `aip-validation`'s wire shape so a service can swap a
                    // hand-rolled `Validator` for the reflective validator without
                    // changing what clients see (ADR-0007).
                    HashMap::from([("fields".to_owned(), paths.join(", "))]),
                    paths
                        .iter()
                        .map(|path| (path.clone(), "field is required".to_owned()))
                        .collect(),
                ),
                Error::ImmutableField { path } => (
                    "FIELD_IMMUTABLE",
                    HashMap::from([("path".to_owned(), path.clone())]),
                    vec![(path.clone(), "immutable field cannot be changed".to_owned())],
                ),
                Error::TypeMismatch { old, updated } => (
                    "FIELD_BEHAVIOR_TYPE_MISMATCH",
                    HashMap::from([
                        ("old".to_owned(), old.clone()),
                        ("updated".to_owned(), updated.clone()),
                    ]),
                    Vec::new(),
                ),
            };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        for (field, description) in violations {
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
    fn required_field_attaches_bad_request_violation() {
        let status: tonic::Status = Error::RequiredFields {
            paths: vec!["display_name".to_owned()],
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "FIELD_REQUIRED");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("fields").map(String::as_str),
            Some("display_name")
        );

        let bad = status
            .get_details_bad_request()
            .expect("BadRequest attached for path errors");
        assert_eq!(bad.field_violations.len(), 1);
        assert_eq!(bad.field_violations[0].field, "display_name");
    }

    #[test]
    fn required_fields_accumulate_one_violation_per_path() {
        // Several missing REQUIRED fields map to one BadRequest per path plus a
        // single comma-joined `fields` metadata key (ADR-0007).
        let status: tonic::Status = Error::RequiredFields {
            paths: vec!["origin_site".to_owned(), "destination_site".to_owned()],
        }
        .into();

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(
            info.metadata.get("fields").map(String::as_str),
            Some("origin_site, destination_site")
        );

        let bad = status
            .get_details_bad_request()
            .expect("BadRequest attached for path errors");
        let fields: Vec<&str> = bad
            .field_violations
            .iter()
            .map(|v| v.field.as_str())
            .collect();
        assert_eq!(fields, ["origin_site", "destination_site"]);
    }

    #[test]
    fn conversion_stamps_the_aip_rs_sentinel_domain() {
        // The pre-boundary contract: `From<Error>` stamps the library sentinel
        // `aip-rs`, carrying `reason` and the `BadRequest` it always has. A
        // deploying service rewrites the sentinel to its own domain at the
        // serving edge with the `aip-errordomain` layer (ADR-0007), not here.
        let status: tonic::Status = Error::RequiredFields {
            paths: vec!["display_name".to_owned()],
        }
        .into();
        let info = status.get_details_error_info().expect("ErrorInfo attached");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.reason, "FIELD_REQUIRED");
        let bad = status
            .get_details_bad_request()
            .expect("BadRequest attached");
        assert_eq!(bad.field_violations[0].field, "display_name");
    }

    #[test]
    fn immutable_field_attaches_bad_request_violation() {
        let status: tonic::Status = Error::ImmutableField {
            path: "external_reference_id".to_owned(),
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "FIELD_IMMUTABLE");
        assert_eq!(info.domain, ERROR_DOMAIN);
        // `validate_immutable_not_changed` keeps bail-on-first, so it stays a
        // single-path error mirrored under `path` (ADR-0007).

        let bad = status
            .get_details_bad_request()
            .expect("BadRequest attached for path errors");
        assert_eq!(bad.field_violations[0].field, "external_reference_id");
    }

    #[test]
    fn type_mismatch_has_error_info_but_no_bad_request() {
        let status: tonic::Status = Error::TypeMismatch {
            old: "Foo".to_owned(),
            updated: "Bar".to_owned(),
        }
        .into();
        let info = status
            .get_details_error_info()
            .expect("ErrorInfo always attached (AIP-193)");
        assert_eq!(info.reason, "FIELD_BEHAVIOR_TYPE_MISMATCH");
        assert!(status.get_details_bad_request().is_none());
    }
}
