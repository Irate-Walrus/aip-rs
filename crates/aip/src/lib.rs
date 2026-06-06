//! aip — a Rust SDK for Google's API Improvement Proposals (AIP).
//!
//! Umbrella crate that re-exports the per-feature crates. All features are on by
//! default; disable default features and pick only what you need:
//!
//! ```toml
//! aip = { version = "0.1", default-features = false, features = ["resourcename"] }
//! ```

#[cfg(feature = "resourcename")]
pub use aip_resourcename as resourcename;

#[cfg(feature = "resourceid")]
pub use aip_resourceid as resourceid;

#[cfg(feature = "pagination")]
pub use aip_pagination as pagination;

#[cfg(feature = "fieldmask")]
pub use aip_fieldmask as fieldmask;

#[cfg(feature = "ordering")]
pub use aip_ordering as ordering;

#[cfg(feature = "filtering")]
pub use aip_filtering as filtering;
