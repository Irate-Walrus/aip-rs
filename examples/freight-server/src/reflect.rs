//! Reflection bridge between the generated prost types and `DynamicMessage`.
//!
//! Several aip-rs primitives are reflective — they operate on
//! `prost_reflect::DynamicMessage`, not concrete generated types. This module
//! transcodes a generated message to/from a `DynamicMessage` (through its
//! protobuf wire bytes) using the shared [`DESCRIPTOR_POOL`], so a handler can
//! checksum a request or apply a field mask and get a concrete message back.
//!
//! [`DESCRIPTOR_POOL`]: crate::proto::DESCRIPTOR_POOL

use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor};

/// The reflective [`MessageDescriptor`] for a freight message by its
/// fully-qualified name (e.g. `einride.example.freight.v1.Shipper`).
pub fn descriptor(full_name: &str) -> MessageDescriptor {
    crate::proto::DESCRIPTOR_POOL
        .get_message_by_name(full_name)
        .unwrap_or_else(|| panic!("`{full_name}` is in the freight descriptor pool"))
}

/// Transcode a concrete generated message into a [`DynamicMessage`] of
/// `descriptor`, through its wire encoding.
pub fn to_dynamic(descriptor: &MessageDescriptor, message: &impl Message) -> DynamicMessage {
    DynamicMessage::decode(descriptor.clone(), message.encode_to_vec().as_slice())
        .expect("a generated message decodes into its own descriptor")
}

/// Transcode a [`DynamicMessage`] back into a concrete generated message,
/// through its wire encoding.
pub fn from_dynamic<T: Message + Default>(message: &DynamicMessage) -> T {
    T::decode(message.encode_to_vec().as_slice())
        .expect("a DynamicMessage re-decodes into its generated type")
}
