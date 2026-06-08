//! Validate a message's `google.api.resource_reference` fields. Ports aip-go's
//! `aipreflect.ValidateResourceReferences`.
//!
//! For every string (or repeated-string) field carrying a
//! `google.api.resource_reference`, the field value must be a valid name of the
//! referenced resource type — i.e. it must match one of that resource's declared
//! patterns. The referenced resource is resolved among the
//! [`ResourceDescriptor`](crate::ResourceDescriptor)s of the field's own package
//! (matching aip-go's package-scoped lookup).
//!
//! Faithful to aip-go, a reference whose type is **not declared** in that package
//! — including a `child_type`/`*` reference, which [`resource_reference_type`]
//! reports as absent — is left unvalidated rather than rejected. Only a value
//! that fails to match a *declared* resource's patterns is an error.
//!
//! Map fields are not traversed: `resource_reference` is carried by scalar and
//! repeated string fields, never by map values, across the AIP surface this
//! supports.

use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, ReflectMessage, Value};

use crate::resources::{
    resource_descriptors_in_package, resource_reference_type, ResourceDescriptor,
};
use crate::Error;

/// Validates the `google.api.resource_reference` fields of `message`. The
/// headline **Typed facade** over [`validate_resource_references_dynamic`]
/// (ADR-0009).
///
/// Use this on inbound request messages to reject a resource reference that does
/// not name the expected resource type. The `M → DynamicMessage` transcode is
/// infallible by construction (ADR-0009).
pub fn validate_resource_references<M: ReflectMessage>(message: &M) -> Result<(), Error> {
    let descriptor = message.descriptor();
    let dynamic = DynamicMessage::decode(descriptor, message.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    validate_resource_references_dynamic(&dynamic)
}

/// The [Dynamic core](crate) of the resource-reference validator: works on a
/// [`DynamicMessage`] directly (ADR-0009). The escape hatch for callers that
/// already hold a dynamic message, and this crate's test surface.
pub fn validate_resource_references_dynamic(message: &DynamicMessage) -> Result<(), Error> {
    validate_in(message, "")
}

/// Recursively validates the resource-reference fields of `message`, threading
/// the field path (`prefix`) so errors point at e.g. `shipment.origin_site` or
/// `names[1]`.
fn validate_in(message: &DynamicMessage, prefix: &str) -> Result<(), Error> {
    for field in message.descriptor().fields() {
        let base = join_path(prefix, field.name());

        if field.is_map() {
            // Maps are not traversed — see the module note.
            continue;
        }

        if field.is_list() {
            match field.kind() {
                Kind::String => {
                    if let Some(resource_type) = resource_reference_type(&field) {
                        let value = message.get_field(&field);
                        if let Value::List(list) = value.as_ref() {
                            // Resolve the referenced resource once for the whole
                            // list rather than per element (a batch can carry
                            // hundreds of names).
                            let resource = referenced_resource(&field, &resource_type);
                            for (i, item) in list.iter().enumerate() {
                                if let Some(name) = item.as_str() {
                                    check_reference(
                                        resource.as_ref(),
                                        &resource_type,
                                        name,
                                        &format!("{base}[{i}]"),
                                    )?;
                                }
                            }
                        }
                    }
                }
                Kind::Message(_) => {
                    let value = message.get_field(&field);
                    if let Value::List(list) = value.as_ref() {
                        for (i, item) in list.iter().enumerate() {
                            if let Value::Message(nested) = item {
                                validate_in(nested, &format!("{base}[{i}]"))?;
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        match field.kind() {
            Kind::String => {
                if let Some(resource_type) = resource_reference_type(&field) {
                    // proto3 scalar presence: an unset (empty) string is absent
                    // and not visited, matching aip-go's reflective walk.
                    if message.has_field(&field) {
                        let value = message.get_field(&field);
                        if let Some(name) = value.as_str() {
                            let resource = referenced_resource(&field, &resource_type);
                            check_reference(resource.as_ref(), &resource_type, name, &base)?;
                        }
                    }
                }
            }
            Kind::Message(_) if message.has_field(&field) => {
                let value = message.get_field(&field);
                if let Value::Message(nested) = value.as_ref() {
                    validate_in(nested, &base)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The resource declared in `field`'s package whose type is `resource_type`, if
/// any — aip-go's package-scoped lookup. `None` when the type is not declared in
/// scope (including `child_type`/`*`/cross-package references), which leaves the
/// reference unvalidated.
fn referenced_resource(field: &FieldDescriptor, resource_type: &str) -> Option<ResourceDescriptor> {
    let file = field.parent_file();
    resource_descriptors_in_package(field.parent_pool(), file.package_name())
        .into_iter()
        .find(|resource| resource.resource_type == resource_type)
}

/// Checks that `name` is a valid reference: it must match one of `resource`'s
/// declared patterns. An undeclared type (`resource` is `None`) passes
/// unvalidated (see the module note).
fn check_reference(
    resource: Option<&ResourceDescriptor>,
    resource_type: &str,
    name: &str,
    path: &str,
) -> Result<(), Error> {
    let Some(resource) = resource else {
        return Ok(());
    };
    if resource
        .patterns
        .iter()
        .any(|pattern| aip_resourcename::is_match(pattern, name))
    {
        return Ok(());
    }
    Err(Error::InvalidResourceReference {
        field: path.to_owned(),
        value: name.to_owned(),
        resource_type: resource_type.to_owned(),
    })
}

/// Joins a path prefix with a field name using `.`, or returns the name when the
/// prefix is empty.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}
