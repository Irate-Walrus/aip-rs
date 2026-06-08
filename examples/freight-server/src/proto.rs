//! Generated protobuf types and the `FreightService` server trait.
//!
//! `build.rs` writes one Rust file per proto package into `OUT_DIR`; we mount
//! them in a module tree that mirrors each package path so prost's cross-package
//! references resolve. `google.protobuf.*` well-known types are mapped to
//! [`prost_types`] (not generated), and `google.api.*` is options-only — neither
//! is referenced by the generated freight field types, so neither is mounted.
// `dead_code`: the generated `google.iam.v1` set carries types the demo never
// constructs (audit configs, policy deltas), which a binary crate flags as unused.
#![allow(clippy::all, missing_docs, rustdoc::all, dead_code)]

use std::sync::LazyLock;

use prost_reflect::DescriptorPool;

/// The freight `FileDescriptorSet` emitted by `build.rs`, embedded so the
/// generated messages can resolve their own descriptors at runtime (the
/// [`ReflectMessage`](prost_reflect::ReflectMessage) derives read [`DESCRIPTOR_POOL`]).
static FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/freight_descriptor_set.bin"));

/// Shared [`DescriptorPool`] over the freight protos. Backs the generated
/// `ReflectMessage` derives — every Typed message resolves its own
/// `MessageDescriptor` from this pool (ADR-0009); cheaply cloned (it is
/// reference-counted internally).
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
        // Holds `LatLng` (freight `Site`) and `Expr` (the IAM `Binding.condition`).
        include!(concat!(env!("OUT_DIR"), "/google.r#type.rs"));
    }
    pub mod iam {
        pub mod v1 {
            // The `IAMPolicy` service trait + its `Policy` / `Binding` / request
            // messages (aip #64). `Binding.condition` resolves to the sibling
            // `super::super::r#type::Expr` mounted above.
            include!(concat!(env!("OUT_DIR"), "/google.iam.v1.rs"));
        }
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
