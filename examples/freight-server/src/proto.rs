//! Generated protobuf types and the `FreightService` server trait.
//!
//! `build.rs` writes one Rust file per proto package into `OUT_DIR`; we mount
//! them in a module tree that mirrors each package path so prost's cross-package
//! references resolve. `google.protobuf.*` well-known types are mapped to
//! [`prost_types`] (not generated), and `google.api.*` is options-only — neither
//! is referenced by the generated freight field types, so neither is mounted.
#![allow(clippy::all, missing_docs, rustdoc::all)]

use std::sync::LazyLock;

use prost_reflect::DescriptorPool;

/// The freight `FileDescriptorSet` emitted by `build.rs`, embedded for runtime
/// reflection (the reflective `aip::pagination::request_checksum` needs a
/// descriptor pool).
static FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/freight_descriptor_set.bin"));

/// Shared [`DescriptorPool`] over the freight protos. Used to transcode a
/// concrete generated request into a `DynamicMessage` for reflective AIP
/// primitives; cheaply cloned (it is reference-counted internally).
pub static DESCRIPTOR_POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(FILE_DESCRIPTOR_SET)
        .expect("the embedded freight descriptor set is well-formed")
});

pub mod einride {
    pub mod example {
        pub mod freight {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/einride.example.freight.v1.rs"));
            }
        }
    }
}

pub mod google {
    pub mod r#type {
        // prost escapes the `type` keyword in the generated file name, too.
        include!(concat!(env!("OUT_DIR"), "/google.r#type.rs"));
    }
}

#[cfg(test)]
mod tests {
    use prost_reflect::ReflectMessage;

    use super::einride::example::freight::v1::{ListShippersRequest, Shipper};

    /// ADR-0009 smoke check: a generated freight type is a **Typed message** — it
    /// resolves its own `MessageDescriptor` straight off the value, with no
    /// hand-built pool lookup at the call site (`DESCRIPTOR_POOL.get_message_by_name`).
    #[test]
    fn generated_types_carry_their_descriptor() {
        assert_eq!(
            Shipper::default().descriptor().full_name(),
            "einride.example.freight.v1.Shipper"
        );
        assert_eq!(
            ListShippersRequest::default().descriptor().full_name(),
            "einride.example.freight.v1.ListShippersRequest"
        );
    }
}
