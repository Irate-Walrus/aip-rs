//! Test-only fixture harness for aip-rs's reflective crates.
//!
//! The vendored einride example protos (`einride.example.freight.v1` and
//! `einride.example.syntax.v1`) and their googleapis imports are compiled with
//! [`protox`] at build time — **no `protoc` required** — and embedded as a
//! `FileDescriptorSet`. This crate exposes that set as a shared
//! [`DescriptorPool`] and provides [`from_json`], which builds a
//! [`DynamicMessage`] of a named message type from a JSON string.
//!
//! ```
//! let shipper = test_fixtures::from_json(
//!     "einride.example.freight.v1.Shipper",
//!     r#"{"name": "shippers/acme", "displayName": "Acme"}"#,
//! )
//! .unwrap();
//! let name = shipper.get_field_by_name("name").unwrap();
//! assert_eq!(name.as_str(), Some("shippers/acme"));
//! ```
//!
//! [`protox`]: https://crates.io/crates/protox

use std::sync::LazyLock;

use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};

/// The `FileDescriptorSet` produced by `build.rs`, embedded into the test binary.
static FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/file_descriptor_set.bin"));

static POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(FILE_DESCRIPTOR_SET)
        .expect("the embedded file descriptor set is well-formed")
});

/// Errors building a [`DynamicMessage`] fixture.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    /// No message type with the given fully-qualified name exists in the pool.
    #[error("message type `{0}` is not in the fixture descriptor pool")]
    UnknownMessage(String),

    /// The JSON could not be deserialized into the named message type.
    #[error("failed to deserialize `{message}` from JSON: {source}")]
    Json {
        message: String,
        #[source]
        source: serde_json::Error,
    },
}

/// The shared [`DescriptorPool`] holding every vendored example proto and its
/// imports. Cheaply cloned (it is reference-counted internally), so reflective
/// crates can call `pool()` freely from their tests.
pub fn pool() -> DescriptorPool {
    POOL.clone()
}

/// Look up a message type by its fully-qualified name (e.g.
/// `"einride.example.freight.v1.Shipper"`) in the shared [`pool`].
pub fn message_descriptor(full_name: &str) -> Option<MessageDescriptor> {
    POOL.get_message_by_name(full_name)
}

/// Build a [`DynamicMessage`] of the named message type from a JSON string,
/// using the canonical protobuf JSON mapping (e.g. `createTime` for a
/// `create_time` field, an RFC 3339 string for a `Timestamp`).
///
/// `full_name` is the fully-qualified message name, such as
/// `"einride.example.freight.v1.Shipper"`.
pub fn from_json(full_name: &str, json: &str) -> Result<DynamicMessage, FixtureError> {
    let descriptor = message_descriptor(full_name)
        .ok_or_else(|| FixtureError::UnknownMessage(full_name.to_owned()))?;

    let mut deserializer = serde_json::Deserializer::from_str(json);
    let message = DynamicMessage::deserialize(descriptor, &mut deserializer).map_err(|source| {
        FixtureError::Json {
            message: full_name.to_owned(),
            source,
        }
    })?;
    deserializer.end().map_err(|source| FixtureError::Json {
        message: full_name.to_owned(),
        source,
    })?;

    Ok(message)
}
