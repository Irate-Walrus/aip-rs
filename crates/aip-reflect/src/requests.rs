//! Iterate **Request descriptors** over the top-level messages of a protobuf
//! file: which AIP-standard request fields (`page_token`, `page_size`, `skip`,
//! `order_by`, `filter`) each message carries (ADR-0013).
//!
//! These drive the codegen plugin's request-trait emission — `aip-codegen`
//! emits e.g. an `aip_pagination::PageRequest` impl iff the message has the
//! fields the trait reads, the Rust analog of aip-go's structural trait
//! satisfaction. A field counts only when its name **and** type match and it
//! is not proto3-`optional` (prost maps `optional int32 page_size` to
//! `Option<i32>`, which a `fn page_size(&self) -> i32` body cannot return); a
//! near-miss is silently `false`, never an error. Nested messages are not
//! walked (matching the resource iteration).

use prost_reflect::{FileDescriptor, Kind, MessageDescriptor};

/// A **Request descriptor**: an aip-rs digest of AIP-standard request-field
/// presence over one top-level message (not a `prost_reflect`
/// `MessageDescriptor`). See ADR-0013.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestDescriptor {
    /// The message's simple name, e.g. `ListSitesRequest` — also the name of
    /// the prost-generated struct (AIP request messages are `PascalCase`).
    pub message_name: String,
    /// The message has a plain `string page_token` field (AIP-158).
    pub has_page_token: bool,
    /// The message has a plain `int32 page_size` field (AIP-158).
    pub has_page_size: bool,
    /// The message has a plain `int32 skip` field (AIP-158).
    pub has_skip: bool,
    /// The message has a plain `string order_by` field (AIP-132).
    pub has_order_by: bool,
    /// The message has a plain `string filter` field (AIP-160).
    pub has_filter: bool,
}

/// Enumerates the [`RequestDescriptor`] of every top-level message in `file`
/// (nested messages are not walked, matching
/// [`resource_descriptors_in_file`](crate::resource_descriptors_in_file)).
pub fn request_descriptors_in_file(file: &FileDescriptor) -> Vec<RequestDescriptor> {
    file.messages().map(|m| request_descriptor(&m)).collect()
}

/// Digests one message's AIP-standard request fields.
fn request_descriptor(message: &MessageDescriptor) -> RequestDescriptor {
    RequestDescriptor {
        message_name: message.name().to_owned(),
        has_page_token: has_scalar_field(message, "page_token", &Kind::String),
        has_page_size: has_scalar_field(message, "page_size", &Kind::Int32),
        has_skip: has_scalar_field(message, "skip", &Kind::Int32),
        has_order_by: has_scalar_field(message, "order_by", &Kind::String),
        has_filter: has_scalar_field(message, "filter", &Kind::String),
    }
}

/// Whether `message` has a singular, non-`optional` field named `name` of
/// scalar kind `kind` — the shape a by-value accessor (`fn page_size(&self) ->
/// i32`) can serve. `supports_presence` is what excludes proto3-`optional`
/// (prost generates `Option<_>` for those); it cannot mask a message kind here
/// because `kind` is scalar.
fn has_scalar_field(message: &MessageDescriptor, name: &str, kind: &Kind) -> bool {
    message
        .get_field_by_name(name)
        .is_some_and(|f| &f.kind() == kind && !f.is_list() && !f.supports_presence())
}
