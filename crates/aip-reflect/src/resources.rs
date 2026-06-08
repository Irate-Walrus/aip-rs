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
    DescriptorPool, DynamicMessage, FieldDescriptor, FileDescriptor, MessageDescriptor, Value,
};

/// Field number of the `google.api.resource` extension on `MessageOptions`.
const RESOURCE_TAG: u32 = 1053;
/// Field number of the `google.api.resource_definition` extension on `FileOptions`.
const RESOURCE_DEFINITION_TAG: u32 = 1053;
/// Field number of the `google.api.resource_reference` extension on `FieldOptions`.
const RESOURCE_REFERENCE_TAG: u32 = 1055;

/// A `google.api.resource` descriptor, reduced to the fields this library uses:
/// the resource `type` and its `pattern`s. (aip-go's `ResourceDescriptor` also
/// carries `singular`/`plural`/`name_field`/… which only the code generator
/// needs; those are deferred to that work, issue #62.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDescriptor {
    /// The resource type, e.g. `freight-example.einride.tech/Shipper`.
    pub resource_type: String,
    /// The resource name patterns, e.g. `["shippers/{shipper}"]`.
    pub patterns: Vec<String>,
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
        Value::Message(resource) => Some(resource_descriptor_from_message(resource)),
        _ => None,
    }
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
