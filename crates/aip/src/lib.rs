//! `aip` — a Rust SDK for Google's [API Improvement Proposals][aip-dev] (AIP).
//!
//! Per-feature crates re-exported under one umbrella: enable every crate by
//! taking the default features, or pick exactly what you need:
//!
//! ```toml
//! # Full SDK (default).
//! aip = "0.1"
//!
//! # Minimal: only resource-name parsing.
//! aip = { version = "0.1", default-features = false, features = ["resourcename"] }
//! ```
//!
//! # The `tonic` feature
//!
//! The optional `tonic` feature is the switch that turns AIP errors into
//! rich gRPC statuses. When it is enabled every crate's `Error` type gets
//! `From<Error> for tonic::Status`, mapping each failure to the correct gRPC
//! code (`INVALID_ARGUMENT`, `ABORTED`, `NOT_FOUND`, …) with
//! [AIP-193][aip-193] standard error details attached (`ErrorInfo` +
//! `BadRequest`). A bare `?` in a tonic handler then produces the right
//! status automatically:
//!
//! ```toml
//! aip = { version = "0.1", features = ["tonic"] }
//! ```
//!
//! # Worked example
//!
//! A compact update handler exercising pagination preamble, field-mask
//! application, etag freshness check, and the `?`-to-`Status` conversion:
//!
//! ```no_run
//! # use tonic::{Request, Response, Status};
//! # use prost_reflect::ReflectMessage as _;
//! # use aip::pagination::{Page, SizeLimits};
//! # struct MyResource { name: String, etag: String, display_name: String }
//! # impl prost_reflect::ReflectMessage for MyResource {
//! #     fn descriptor(&self) -> prost_reflect::MessageDescriptor { unimplemented!() }
//! # }
//! # impl prost::Message for MyResource {
//! #     fn encode_raw(&self, _: &mut impl prost::bytes::BufMut) {}
//! #     fn merge_field(&mut self, _: u32, _: prost_reflect::prost::encoding::WireType,
//! #                   _: &mut impl prost::bytes::Buf, _: prost::encoding::DecodeContext) -> Result<(), prost::DecodeError> { Ok(()) }
//! #     fn encoded_len(&self) -> usize { 0 }
//! #     fn clear(&mut self) {}
//! # }
//! # #[derive(Default)] struct ListReq { page_size: i32, page_token: String, filter: String }
//! # impl aip::pagination::PageRequest for ListReq {
//! #     fn page_size(&self) -> i32 { self.page_size }
//! #     fn page_token(&self) -> &str { &self.page_token }
//! #     fn skip(&self) -> i32 { 0 }
//! # }
//! # impl prost_reflect::ReflectMessage for ListReq {
//! #     fn descriptor(&self) -> prost_reflect::MessageDescriptor { unimplemented!() }
//! # }
//! # struct UpdateReq { resource: Option<MyResource>, update_mask: Option<prost_types::FieldMask>, etag: String, validate_only: bool }
//! # fn store_get(_: &str) -> Option<MyResource> { None }
//! # fn store_put(_: MyResource) {}
//! # fn now_str() -> String { String::new() }
//!
//! const PAGE_LIMITS: SizeLimits = SizeLimits { default: 50, max: 1000 };
//!
//! // List: pagination preamble (AIP-158) — validates + resolves page size,
//! // verifies the token's request checksum so mid-pagination changes are caught.
//! fn list(req: Request<ListReq>) -> Result<Response<Vec<MyResource>>, Status> {
//!     let req = req.into_inner();
//!     let page = Page::parse(&req, PAGE_LIMITS)?;          // ? -> INVALID_ARGUMENT
//!     let all: Vec<MyResource> = vec![/* … */];
//!     let (items, next_page_token) = page.apply(all);
//!     Ok(Response::new(items))
//! }
//!
//! // Update: etag freshness + field-mask apply + validate_only gate (AIP-134/154/163).
//! fn update(req: Request<UpdateReq>) -> Result<Response<MyResource>, Status> {
//!     let req = req.into_inner();
//!     let incoming = req.resource.ok_or_else(|| Status::invalid_argument("resource required"))?;
//!     let existing = store_get(&incoming.name)
//!         .ok_or_else(|| Status::not_found(format!("{} not found", incoming.name)))?;
//!
//!     // AIP-154: verify the client's etag before touching the store.
//!     // A stale etag -> ABORTED; malformed -> INVALID_ARGUMENT.
//!     aip::etag::check(&incoming.etag, &existing)?;
//!
//!     // AIP-134: apply the update mask (validates paths, merges fields).
//!     let mask = req.update_mask.unwrap_or_default();
//!     aip::fieldmask::validate(&mask, &MyResource { name: String::new(), etag: String::new(), display_name: String::new() }.descriptor())?;
//!     let mut resource = existing;
//!     aip::fieldmask::update(&mask, &mut resource, &incoming)?;
//!
//!     // AIP-163: skip the store write on validate_only requests.
//!     aip::preview::commit_unless(req.validate_only, || store_put(resource.clone()));
//!     Ok(Response::new(resource))
//! }
//! ```
//!
//! For a full end-to-end example — pagination, soft delete, IAM, request IDs,
//! field-behavior validation — see [`examples/freight-server`][freight-server].
//!
//! [aip-dev]: https://google.aip.dev
//! [aip-193]: https://google.aip.dev/193
//! [freight-server]: https://github.com/Irate-Walrus/aip-rs/tree/main/examples/freight-server

#![cfg_attr(docsrs, feature(doc_cfg))]

/// AIP-122 resource names: parse, format, match, and validate
/// `/`-separated resource name strings.
///
/// Key types: [`Pattern`](aip_resourcename::Pattern),
/// [`Captures`](aip_resourcename::Captures).
///
/// See <https://google.aip.dev/122>.
#[cfg(feature = "resourcename")]
#[cfg_attr(docsrs, doc(cfg(feature = "resourcename")))]
pub use aip_resourcename as resourcename;

/// AIP-122 resource IDs: validate user-settable IDs and generate system IDs.
///
/// Key fn: [`validate_user_settable`](aip_resourceid::validate_user_settable),
/// [`generate`](aip_resourceid::generate).
///
/// See <https://google.aip.dev/122> and <https://google.aip.dev/148>.
#[cfg(feature = "resourceid")]
#[cfg_attr(docsrs, doc(cfg(feature = "resourceid")))]
pub use aip_resourceid as resourceid;

/// AIP-155 request identification: validate a `request_id` and name the
/// idempotency [`Replay`](aip_requestid::Replay) contract a server enforces
/// over its cache of seen ids.
///
/// See <https://google.aip.dev/155>.
#[cfg(feature = "requestid")]
#[cfg_attr(docsrs, doc(cfg(feature = "requestid")))]
pub use aip_requestid as requestid;

/// AIP-158 pagination: encode/decode page tokens and resolve page size against
/// server-side limits.
///
/// Key types: [`Page`](aip_pagination::Page),
/// [`SizeLimits`](aip_pagination::SizeLimits).
///
/// See <https://google.aip.dev/158>.
#[cfg(feature = "pagination")]
#[cfg_attr(docsrs, doc(cfg(feature = "pagination")))]
pub use aip_pagination as pagination;

/// AIP-134/161 field masks: apply update masks and validate paths against a
/// message descriptor.
///
/// Key fns: [`validate`](aip_fieldmask::validate),
/// [`update`](aip_fieldmask::update).
///
/// See <https://google.aip.dev/134> and <https://google.aip.dev/161>.
#[cfg(feature = "fieldmask")]
#[cfg_attr(docsrs, doc(cfg(feature = "fieldmask")))]
pub use aip_fieldmask as fieldmask;

/// AIP-132 ordering: parse and validate `order_by` expressions.
///
/// Key type: [`OrderBy`](aip_ordering::OrderBy).
///
/// See <https://google.aip.dev/132>.
#[cfg(feature = "ordering")]
#[cfg_attr(docsrs, doc(cfg(feature = "ordering")))]
pub use aip_ordering as ordering;

/// AIP-160 filtering: parse and type-check filter expressions into a native
/// AST; optional in-memory matcher and SQL transpilation.
///
/// Key fns: [`parse`](aip_filtering::parse),
/// [`check`](aip_filtering::check).
///
/// See <https://google.aip.dev/160>.
#[cfg(feature = "filtering")]
#[cfg_attr(docsrs, doc(cfg(feature = "filtering")))]
pub use aip_filtering as filtering;

/// AIP-161/203 field behavior: read, clear, copy, and validate fields by their
/// `google.api.field_behavior` annotation (`REQUIRED`, `OUTPUT_ONLY`,
/// `IMMUTABLE`).
///
/// Key fns: [`validate_required`](aip_fieldbehavior::validate_required),
/// [`clear_fields`](aip_fieldbehavior::clear_fields),
/// [`copy_fields`](aip_fieldbehavior::copy_fields).
///
/// See <https://google.aip.dev/161> and <https://google.aip.dev/203>.
#[cfg(feature = "fieldbehavior")]
#[cfg_attr(docsrs, doc(cfg(feature = "fieldbehavior")))]
pub use aip_fieldbehavior as fieldbehavior;

/// AIP-193 error-domain boundary layer — pulled in by the `tonic` feature:
/// [`Layer`](aip_errordomain::Layer) rewrites the library-internal sentinel
/// domain in `grpc-status-details-bin` to the deploying service's own domain,
/// so the service presents one `ErrorInfo.domain` to its clients.
///
/// See <https://google.aip.dev/193>.
#[cfg(feature = "errordomain")]
#[cfg_attr(docsrs, doc(cfg(feature = "errordomain")))]
pub use aip_errordomain as errordomain;

/// AIP-154 content etags: optimistic-concurrency tokens for the
/// read-modify-write cycle over any resource.
///
/// Key fns: [`compute`](aip_etag::compute),
/// [`check`](aip_etag::check).
///
/// See <https://google.aip.dev/154>.
#[cfg(feature = "etag")]
#[cfg_attr(docsrs, doc(cfg(feature = "etag")))]
pub use aip_etag as etag;

/// AIP-164 soft delete / AIP-165 purge: visibility rules and purge contract.
///
/// Key fns: [`is_visible`](aip_softdelete::is_visible),
/// [`check_visible`](aip_softdelete::check_visible),
/// [`check_deleted`](aip_softdelete::check_deleted),
/// [`require_filter`](aip_softdelete::require_filter).
///
/// See <https://google.aip.dev/164> and <https://google.aip.dev/165>.
#[cfg(feature = "softdelete")]
#[cfg_attr(docsrs, doc(cfg(feature = "softdelete")))]
pub use aip_softdelete as softdelete;

/// AIP-163 `validate_only` preview gate:
/// [`commit_unless`](aip_preview::commit_unless) skips the store write on
/// preview requests while the handler still returns the would-be resource.
///
/// See <https://google.aip.dev/163>.
#[cfg(feature = "preview")]
#[cfg_attr(docsrs, doc(cfg(feature = "preview")))]
pub use aip_preview as preview;

/// AIP-193 field-violation accumulator: collect per-field validation failures
/// into one rich gRPC status with `BadRequest` + `ErrorInfo` details.
///
/// Key type: [`Violations`](aip_validation::Violations).
///
/// See <https://google.aip.dev/193>.
#[cfg(feature = "validation")]
#[cfg_attr(docsrs, doc(cfg(feature = "validation")))]
pub use aip_validation as validation;

/// AIP-123 resource-annotation reflection: parse a
/// [`ResourceType`](aip_reflect::ResourceType), iterate
/// `google.api.resource` descriptors in a file/package, and validate
/// `google.api.resource_reference` fields.
///
/// See <https://google.aip.dev/123>.
#[cfg(feature = "reflect")]
#[cfg_attr(docsrs, doc(cfg(feature = "reflect")))]
pub use aip_reflect as reflect;

/// AIP-211 IAM primitives (opt-in via the non-default `iam` feature): parse
/// and validate the `google.iam.v1` identity vocabulary — Member, Role,
/// Permission.
///
/// See <https://google.aip.dev/211>.
#[cfg(feature = "iam")]
#[cfg_attr(docsrs, doc(cfg(feature = "iam")))]
pub use aip_iam as iam;

/// SQL adapter (opt-in via the non-default `sql` feature): transpile a
/// [`Filter`](aip_filtering::Filter) or [`OrderBy`](aip_ordering::OrderBy)
/// into a parameterized SQL clause tail via a pluggable
/// [`Dialect`](aip_sql::Dialect).
///
/// Not part of the parse/validate core — it stays off by default.
///
/// See <https://google.aip.dev/132> and <https://google.aip.dev/160>.
#[cfg(feature = "sql")]
#[cfg_attr(docsrs, doc(cfg(feature = "sql")))]
pub use aip_sql as sql;
