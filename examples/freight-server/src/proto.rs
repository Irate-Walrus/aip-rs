//! Generated protobuf types and the `FreightService` server trait.
//!
//! `build.rs` writes one Rust file per proto package into `OUT_DIR`; we mount
//! them in a module tree that mirrors each package path so prost's cross-package
//! references resolve. `google.protobuf.*` well-known types are mapped to
//! [`prost_types`] (not generated), and `google.api.*` is options-only — neither
//! is referenced by the generated freight field types, so neither is mounted.
#![allow(clippy::all, missing_docs, rustdoc::all)]

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
