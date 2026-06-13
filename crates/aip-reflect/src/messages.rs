//! Enumerate the messages of a protobuf file that earn a generated
//! `prost_reflect::ReflectMessage` impl (ADR-0009): every message **including
//! nested ones**, minus the synthetic map-entry messages prost emits no struct
//! for.
//!
//! Unlike [`resource_descriptors_in_file`](crate::resource_descriptors_in_file)
//! and [`request_descriptors_in_file`](crate::request_descriptors_in_file) —
//! which read only top-level messages — this walk recurses, because the
//! `ReflectMessage` impl is emitted for *every* generated struct, nested or not.
//! It is the descriptor-reading half only: it hands back each message's proto
//! name and its package-relative name chain, and `aip-codegen` turns that chain
//! into the Rust module path (`Decl.FunctionDecl.Overload` ->
//! `decl::function_decl::Overload`).

use prost_reflect::{FileDescriptor, MessageDescriptor};

/// A message that earns a generated `ReflectMessage` impl: its proto name and
/// the package-relative name chain that locates its prost struct. Named to avoid
/// the reserved **Descriptor** term (this is an aip-rs digest, not a
/// `prost_reflect::MessageDescriptor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectMessageName {
    /// The fully-qualified proto name, e.g. `einride.example.freight.v1.Shipper`
    /// or `google.api.expr.v1alpha1.Decl.FunctionDecl.Overload`. Passed verbatim
    /// to `DescriptorPool::get_message_by_name` in the emitted impl.
    pub full_name: String,
    /// The name chain relative to the package, parent-before-child, e.g.
    /// `["Shipper"]` or `["Decl", "FunctionDecl", "Overload"]`. `aip-codegen`
    /// snake-cases the parents into modules and keeps the leaf as the struct
    /// name.
    pub path: Vec<String>,
}

/// Enumerates every [`ReflectMessageName`] in `file`, recursing into nested
/// messages (depth-first, parent before child) and **skipping** synthetic
/// map-entry messages — prost represents a `map<K, V>` as a `HashMap` field, not
/// a generated struct, so there is nothing to impl the trait on.
pub fn reflect_messages_in_file(file: &FileDescriptor) -> Vec<ReflectMessageName> {
    let mut out = Vec::new();
    for message in file.messages() {
        collect(&message, &mut out);
    }
    out
}

/// Pushes `message` (unless it is a map-entry) then recurses its children. A
/// map-entry never has children, so skipping it loses nothing below it.
fn collect(message: &MessageDescriptor, out: &mut Vec<ReflectMessageName>) {
    if !message.is_map_entry() {
        out.push(ReflectMessageName {
            full_name: message.full_name().to_owned(),
            path: relative_path(message),
        });
    }
    for child in message.child_messages() {
        collect(&child, out);
    }
}

/// The package-relative name chain of `message`, parent-before-child. Built by
/// walking `parent_message()` up to the top level rather than string-splitting
/// the full name, so an empty package or a dotted package never confuses it.
fn relative_path(message: &MessageDescriptor) -> Vec<String> {
    let mut chain = vec![message.name().to_owned()];
    let mut current = message.parent_message();
    while let Some(parent) = current {
        chain.push(parent.name().to_owned());
        current = parent.parent_message();
    }
    chain.reverse();
    chain
}
