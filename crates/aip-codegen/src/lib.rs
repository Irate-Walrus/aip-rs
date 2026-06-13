//! Resource-name **code generation** for aip-rs — the analog of aip-go's
//! `protoc-gen-go-aip`, split into a library (this crate) and the thin
//! `protoc-gen-prost-aip` plugin binary that drives it (ADR-0011).
//!
//! This crate holds two things:
//!
//! - The **codegen-only helpers** ported from aip-go's `reflect/aipreflect`:
//!   [`MethodType`], [`GrammaticalName`], and the `strcase` utilities. They
//!   live here, not in the runtime `aip-reflect` crate, because they exist only
//!   to drive generation.
//! - The **generation logic** [`generate`], which walks `google.api.resource`
//!   [`ResourceDescriptor`](aip_reflect::ResourceDescriptor)s and emits typed
//!   resource-name wrappers (e.g. `ShipperResourceName { shipper: String }`)
//!   layered on the runtime [`aip_resourcename::Pattern`] API — it does **not**
//!   duplicate that runtime; the emitted `parse`/`format` call into it — plus an
//!   `impl aip_softdelete::SoftDeletable` on each resource message carrying a
//!   `delete_time` (ADR-0014) — and walks
//!   [`RequestDescriptor`](aip_reflect::RequestDescriptor)s and emits
//!   `impl aip_pagination::PageRequest` keyed on field shape (ADR-0013;
//!   [`MethodType`] is unused by that emission — it stays for the name/grammar
//!   codegen it was ported for).
//!
//! The generator is a pure function over descriptors, so it is unit-tested
//! directly (golden tests) without spawning a `protoc` process; the plugin
//! binary is the only part that touches stdin/stdout.
//!
//! # Example
//!
//! The codegen-only helpers are usable directly; [`generate`] itself is driven
//! by the `protoc-gen-prost-aip` plugin over real descriptors.
//!
//! ```
//! use aip_codegen::{initial_upper_case, GrammaticalName, MethodType};
//!
//! let name = GrammaticalName::new("userEvents");
//! name.validate().unwrap();
//! assert_eq!(name.upper_camel_case(), "UserEvents");
//!
//! assert_eq!(initial_upper_case("shipper"), "Shipper");
//! assert_eq!(MethodType::BatchGet.name_prefix(), "BatchGet");
//! ```

mod generate;
mod grammaticalname;
mod methodtype;
mod strcase;

pub use generate::{generate, CratePaths, GenFile, GenInput};
pub use grammaticalname::GrammaticalName;
pub use methodtype::MethodType;
pub use strcase::initial_upper_case;

/// Errors produced while generating resource-name code.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A [`GrammaticalName`] failed [`validate`](GrammaticalName::validate):
    /// it is empty, not URL-safe, or not `lowerCamelCase`. `reason` carries the
    /// specific failure.
    #[error("invalid grammatical name {name:?}: {reason}")]
    InvalidGrammaticalName { name: String, reason: String },

    /// A `google.api.resource` could not be turned into a wrapper — e.g. its
    /// type has no `service/Type` form, so there is no name for the struct.
    #[error("resource {resource_type:?}: {reason}")]
    InvalidResource {
        resource_type: String,
        reason: String,
    },

    /// A resource name pattern is not a valid [`aip_resourcename::Pattern`] (it
    /// contains a wildcard, a duplicate variable, …), so the generated code
    /// could not parse it at runtime.
    #[error(transparent)]
    Pattern(#[from] aip_resourcename::Error),
}
