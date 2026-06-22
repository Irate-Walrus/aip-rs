//! Generated protobuf types and the `FreightService` / `IAMPolicy` server traits.
//!
//! The code under `src/gen` is emitted by `buf generate` (see `regen.sh`) and
//! **committed** (ADR-0011): there is no codegen `build.rs` and no vendored
//! `google/**`, so building the example needs no proto toolchain. We mount the
//! generated files in a module tree that mirrors each package path so prost's
//! cross-package references resolve. The shared `google.*` types are not
//! generated here at all — `extern_path` maps them onto [`aip_proto`], so e.g.
//! the `google.iam.v1.Policy` the `IAMPolicy` service trait speaks *is* the one
//! `aip::iam`'s structural helpers operate on (one `Policy` type by
//! construction). `google.protobuf.*` well-known types map to [`prost_types`].
//!
//! Each `google.api.resource` annotation also yields a typed resource-name
//! wrapper (`ShipperResourceName`, …) via `protoc-gen-prost-aip` — the proto
//! annotation is the single source of truth for the name pattern (ADR-0011) —
//! and each pagination-shaped request message an `aip_pagination::PageRequest`
//! impl, keyed on its field shape behind `buf.gen.yaml`'s `pagination=true`
//! (ADR-0013).
// `dead_code`: the generated service plumbing and resource-name wrappers carry
// items the demo never constructs, which a binary crate flags as unused.
#![allow(clippy::all, missing_docs, rustdoc::all, dead_code)]

use std::sync::LazyLock;

use prost_reflect::DescriptorPool;

/// The freight `FileDescriptorSet` emitted by `buf build` via `regen.sh` —
/// import-complete, with extension bytes (e.g. `google.api.field_behavior`)
/// preserved — embedded so the generated messages can resolve their own
/// descriptors at runtime (the [`ReflectMessage`](prost_reflect::ReflectMessage)
/// derives read [`DESCRIPTOR_POOL`]).
pub static FILE_DESCRIPTOR_SET: &[u8] = include_bytes!("descriptor_set.binpb");

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
                // The message structs and the `FreightService` server trait.
                include!("gen/einride/example/freight/v1/einride.example.freight.v1.rs");
                include!("gen/einride/example/freight/v1/einride.example.freight.v1.tonic.rs");

                // The per-proto AIP code — typed resource-name wrappers and
                // request-trait impls. The files are `use`-free and fully
                // path-qualified, mounted directly in this message module
                // (ADR-0013's one mount rule): an `impl … for ListSitesRequest`
                // names the prost struct above by bare path, and the wrappers
                // land flat alongside the messages.
                include!("gen/einride/example/freight/v1/freight_service.aip.rs");
                include!("gen/einride/example/freight/v1/shipment.aip.rs");
                include!("gen/einride/example/freight/v1/shipper.aip.rs");
                include!("gen/einride/example/freight/v1/site.aip.rs");
            }
        }
    }
}

pub mod google {
    // Nothing `google.type` is generated or mounted here: the generated freight
    // structs reference `LatLng` by its real path (`aip_proto::google::r#type`),
    // and so does any hand-written code that needs it.
    pub mod iam {
        pub mod v1 {
            // Every `google.iam.v1` *message* (`Policy`, the request/response
            // types, …) comes from aip-proto; only the `IAMPolicy` service
            // trait is generated here, referencing those very types.
            pub use aip_proto::google::iam::v1::*;

            include!("gen/google/iam/v1/google.iam.v1.tonic.rs");
        }
    }

    pub mod longrunning {
        // Every `google.longrunning` *message* (`Operation`, the
        // `GetOperationRequest` / `ListOperationsResponse` / … types) comes from
        // aip-proto — the one `Operation` the `BatchCreateShippers` method returns
        // and the `Operations` service speaks (ADR-0015) — and only the
        // `Operations` service trait is generated here, referencing those types.
        pub use aip_proto::google::longrunning::*;

        include!("gen/google/longrunning/google.longrunning.tonic.rs");
    }
}

#[cfg(test)]
mod tests {
    use prost_reflect::ReflectMessage;

    use super::einride::example::freight::v1::{
        BatchGetSitesRequest, ListShippersRequest, Shipment, Shipper, ShipperResourceName,
    };

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

    /// aip #61: the freight protos' `google.api.resource_reference` annotations
    /// resolve against `aip::reflect::validate_resource_references`, run over the
    /// real generated **Typed messages** (the typed facade, ADR-0009). A
    /// well-formed reference passes; a value that names the wrong resource type
    /// is rejected — proving the primitive bites against the example protos.
    #[test]
    fn freight_resource_references_resolve() {
        // `origin_site`/`destination_site` reference Site; both are valid Sites.
        let shipment = Shipment {
            origin_site: "shippers/1/sites/1".to_owned(),
            destination_site: "shippers/1/sites/2".to_owned(),
            ..Default::default()
        };
        aip::reflect::validate_resource_references(&shipment)
            .expect("both sites are valid Site references");

        // Repeated `names` reference Site, `parent` references Shipper.
        let batch = BatchGetSitesRequest {
            parent: "shippers/1".to_owned(),
            names: vec![
                "shippers/1/sites/1".to_owned(),
                "shippers/1/sites/2".to_owned(),
            ],
        };
        aip::reflect::validate_resource_references(&batch)
            .expect("parent is a Shipper and every name is a Site");

        // `shippers/1` is a Shipper name, not a Site — `origin_site` must reject it.
        let bad = Shipment {
            origin_site: "shippers/1".to_owned(),
            destination_site: "shippers/1/sites/2".to_owned(),
            ..Default::default()
        };
        assert!(
            aip::reflect::validate_resource_references(&bad).is_err(),
            "origin_site does not name a Site",
        );
    }

    /// aip #170: generated wrappers carry `TryFrom<&str>` / `TryFrom<String>`
    /// paired with `FromStr`, so a raw **Resource name** string converts into a
    /// typed wrapper via `.try_into()` — borrowed and owned both work, and a
    /// non-matching name is rejected through the same path (same error as parse).
    #[test]
    fn resource_name_string_converts_via_try_into() {
        // Borrowed form: `&str -> ShipperResourceName`.
        let borrowed: ShipperResourceName =
            "shippers/acme".try_into().expect("a valid shipper name");
        assert_eq!(borrowed.shipper(), "acme");

        // Owned form: `String -> ShipperResourceName`, same value.
        let owned: ShipperResourceName = String::from("shippers/acme")
            .try_into()
            .expect("a valid shipper name");
        assert_eq!(owned, borrowed);

        // Wrong shape — a Site name is not a Shipper — rejected through TryFrom.
        let rejected: Result<ShipperResourceName, _> = "shippers/1/sites/1".try_into();
        assert!(
            rejected.is_err(),
            "a Site name does not match the Shipper pattern"
        );
    }
}
