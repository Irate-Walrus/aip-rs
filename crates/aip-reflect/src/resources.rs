//! Iterate the `google.api.resource` descriptors declared on protobuf
//! descriptors. Ports aip-go's `aipreflect.RangeResourceDescriptorsInFile` /
//! `RangeResourceDescriptorsInPackage`, returning owned [`ResourceDescriptor`]s
//! rather than taking a `Range` callback (idiomatic Rust; the collections are
//! small).
//!
//! Resources can be declared two ways, both of which these helpers collect:
//! - a message-level `option (google.api.resource)` (extension `1053` on
//!   `MessageOptions`), and
//! - a file-level `option (google.api.resource_definition)` (repeated extension
//!   `1053` on `FileOptions`).

use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, FileDescriptor, Kind, MessageDescriptor, Value,
};

/// Field number of the `google.api.resource` extension on `MessageOptions`.
const RESOURCE_TAG: u32 = 1053;
/// Field number of the `google.api.resource_definition` extension on `FileOptions`.
const RESOURCE_DEFINITION_TAG: u32 = 1053;
/// Field number of the `google.api.resource_reference` extension on `FieldOptions`.
const RESOURCE_REFERENCE_TAG: u32 = 1055;
/// Fully-qualified name of the well-known timestamp message — the type a
/// soft-deletable resource's `delete_time` field must carry (AIP-164).
const TIMESTAMP_TYPE: &str = "google.protobuf.Timestamp";

/// A `google.api.resource` descriptor, reduced to the fields this library uses:
/// the resource `type` and its `pattern`s, plus the digest the codegen plugin
/// reads — the owning message's name and whether it carries an AIP-164
/// `delete_time`. (aip-go's `ResourceDescriptor` also carries
/// `singular`/`plural`/`name_field`/… which only the code generator needs;
/// those are deferred to that work, issue #62.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDescriptor {
    /// The resource type, e.g. `freight-example.einride.tech/Shipper`.
    pub resource_type: String,
    /// The resource name patterns, e.g. `["shippers/{shipper}"]`.
    pub patterns: Vec<String>,
    /// The simple name of the message that carries this `google.api.resource`
    /// annotation — also the name of its prost-generated struct, which the
    /// codegen plugin names the `SoftDeletable` impl on (ADR-0014). `None` for a
    /// file-level `resource_definition`, which has no owning message.
    pub message_name: Option<String>,
    /// Whether the annotated message carries a singular, non-repeated
    /// `google.protobuf.Timestamp delete_time` field — the AIP-164 soft-delete
    /// stamp. Resource-anchored: it is `false` for a file-level
    /// `resource_definition`, and a `delete_time` of the wrong type is a silent
    /// `false` (no impl), matching ADR-0013's near-miss precedent.
    pub has_delete_time: bool,
}

/// Enumerates every [`ResourceDescriptor`] declared in `file` — its file-level
/// `resource_definition`s followed by the message-level `resource` on each of
/// its top-level messages (matching aip-go, nested messages are not walked).
pub fn resource_descriptors_in_file(file: &FileDescriptor) -> Vec<ResourceDescriptor> {
    let mut out = resource_definitions(file);
    for message in file.messages() {
        if let Some(resource) = resource_on_message(&message) {
            out.push(resource);
        }
    }
    out
}

/// Enumerates every [`ResourceDescriptor`] declared across all files of
/// `package` in `pool`. Ports aip-go's `RangeResourceDescriptorsInPackage`,
/// with `pool` standing in for its `protoregistry.Files`.
pub fn resource_descriptors_in_package(
    pool: &DescriptorPool,
    package: &str,
) -> Vec<ResourceDescriptor> {
    let mut out = vec![];
    for file in pool.files() {
        if file.package_name() == package {
            out.extend(resource_descriptors_in_file(&file));
        }
    }
    out
}

/// Reads the message-level `google.api.resource` option off `message`, if present.
pub(crate) fn resource_on_message(message: &MessageDescriptor) -> Option<ResourceDescriptor> {
    let ext = message
        .parent_pool()
        .get_message_by_name("google.protobuf.MessageOptions")?
        .get_extension(RESOURCE_TAG)?;
    let options = message.options();
    if !options.has_extension(&ext) {
        return None;
    }
    match options.get_extension(&ext).as_ref() {
        Value::Message(resource) => {
            // The resource is anchored to its owning message: record its name (the
            // prost struct the `SoftDeletable` impl lands on) and whether it
            // carries the AIP-164 `delete_time` stamp.
            let mut descriptor = resource_descriptor_from_message(resource);
            descriptor.message_name = Some(message.name().to_owned());
            descriptor.has_delete_time = has_delete_time(message);
            Some(descriptor)
        }
        _ => None,
    }
}

/// Whether `message` carries a singular, non-repeated `google.protobuf.Timestamp`
/// field named `delete_time` — the AIP-164 soft-delete stamp the codegen plugin
/// keys its `SoftDeletable` emission on. A field of the wrong type, a repeated
/// one, or one inside a real `oneof` is `false`, so a near-miss silently emits no
/// impl rather than failing generation (ADR-0013's precedent).
///
/// The `oneof` exclusion is what keeps the contract honest: prost generates a
/// real-`oneof` member as a variant of a separate enum field, not a top-level
/// `Option`, so the emitted `self.delete_time.is_some()` would not compile. A
/// proto3 `optional` field is a *synthetic* `oneof` and stays a top-level
/// `Option`, so it is kept (mirroring how the request-field detector's
/// `!supports_presence()` drops real oneofs but a message field always supports
/// presence and so cannot reuse that check).
fn has_delete_time(message: &MessageDescriptor) -> bool {
    message.get_field_by_name("delete_time").is_some_and(|f| {
        !f.is_list()
            && f.containing_oneof().is_none_or(|o| o.is_synthetic())
            && matches!(f.kind(), Kind::Message(m) if m.full_name() == TIMESTAMP_TYPE)
    })
}

/// Reads the file-level `google.api.resource_definition` options off `file`.
fn resource_definitions(file: &FileDescriptor) -> Vec<ResourceDescriptor> {
    let Some(ext) = file
        .parent_pool()
        .get_message_by_name("google.protobuf.FileOptions")
        .and_then(|opts| opts.get_extension(RESOURCE_DEFINITION_TAG))
    else {
        return vec![];
    };
    let options = file.options();
    if !options.has_extension(&ext) {
        return vec![];
    }
    match options.get_extension(&ext).as_ref() {
        Value::List(list) => list
            .iter()
            .filter_map(|v| match v {
                Value::Message(resource) => Some(resource_descriptor_from_message(resource)),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// The resource type that `field` references via `google.api.resource_reference`,
/// if it carries one with a non-empty `type`.
///
/// A reference by `child_type` (rather than `type`) yields `None`: like aip-go,
/// only `type` references are checked — a `child_type` or `*` reference has no
/// single resource type to validate against.
pub(crate) fn resource_reference_type(field: &FieldDescriptor) -> Option<String> {
    let ext = field
        .parent_pool()
        .get_message_by_name("google.protobuf.FieldOptions")?
        .get_extension(RESOURCE_REFERENCE_TAG)?;
    let options = field.options();
    if !options.has_extension(&ext) {
        return None;
    }
    let extension = options.get_extension(&ext);
    let Value::Message(reference) = extension.as_ref() else {
        return None;
    };
    let resource_type = string_field(reference, "type");
    (!resource_type.is_empty()).then_some(resource_type)
}

/// Builds a [`ResourceDescriptor`] from a `google.api.ResourceDescriptor`
/// dynamic message, reading its `type` and `pattern` fields.
fn resource_descriptor_from_message(resource: &DynamicMessage) -> ResourceDescriptor {
    let patterns = match resource.get_field_by_name("pattern").as_deref() {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => vec![],
    };
    ResourceDescriptor {
        resource_type: string_field(resource, "type"),
        patterns,
        // Overridden by `resource_on_message` for a message-level resource; a
        // file-level `resource_definition` has no owning message, so it keeps
        // these defaults and never earns a `SoftDeletable` impl.
        message_name: None,
        has_delete_time: false,
    }
}

/// Reads a string field by name from `message`, defaulting to `""`.
fn string_field(message: &DynamicMessage, name: &str) -> String {
    message
        .get_field_by_name(name)
        .as_deref()
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
