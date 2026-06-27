//! AIP-158 pagination: page tokens and request checksums.
//!
//! Page tokens are encoded with `serde` + `postcard` + base64url (a 1-byte
//! version prefix guards against format drift) and are deliberately *not*
//! wire-compatible with `aip-go`'s gob tokens.
//!
//! See <https://google.aip.dev/158>.
//!
//! # Example
//!
//! ```
//! use aip_pagination::{CursorEntry, CursorValue, PageRequest, PageToken};
//!
//! struct ListReq {
//!     page_token: String,
//!     page_size: i32,
//! }
//! impl PageRequest for ListReq {
//!     fn page_token(&self) -> &str {
//!         &self.page_token
//!     }
//!     fn page_size(&self) -> i32 {
//!         self.page_size
//!     }
//! }
//!
//! // first page: an empty token decodes to no cursor (checksum from
//! // `request_checksum` for reflective requests; constant here for brevity)
//! let checksum = 42;
//! let first = ListReq { page_token: String::new(), page_size: 10 };
//! assert!(PageToken::parse(&first, checksum).unwrap().is_none());
//!
//! // mint the next-page token from the last row's key, verify it next request
//! let cursor = vec![CursorEntry {
//!     column: "shipper".to_owned(),
//!     value: CursorValue::Text("acme".to_owned()),
//! }];
//! let token = PageToken::encode(cursor.clone(), checksum);
//! let follow_up = ListReq { page_token: token, page_size: 10 };
//! assert_eq!(PageToken::parse(&follow_up, checksum).unwrap(), Some(cursor));
//!
//! // a request that changed mid-pagination is rejected
//! let stale = ListReq { page_token: PageToken::encode(vec![], checksum), page_size: 10 };
//! assert!(PageToken::parse(&stale, 7).is_err());
//! ```
#![cfg_attr(docsrs, feature(doc_cfg))]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use prost::Message as _;
use prost_reflect::{DynamicMessage, ReflectMessage};
use serde::{Deserialize, Serialize};

/// Version byte prepended to every encoded page token. Bump it whenever the
/// token wire format changes so that tokens minted by an older format fail
/// loudly (see ADR-0004) instead of silently mis-decoding under the new one.
const PAGE_TOKEN_VERSION: u8 = 2;

/// Errors produced when parsing or verifying page tokens.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("malformed page token")]
    Malformed,
    /// The token's version prefix is not one this build understands — it was
    /// minted by a different (typically older) page-token format.
    #[error("unsupported page token version (got {found}, expected {expected})")]
    UnsupportedVersion {
        /// The version byte found in the token.
        found: u8,
        /// The version byte this build emits and accepts.
        expected: u8,
    },
    #[error("page token checksum mismatch (request changed between pages)")]
    ChecksumMismatch,
    #[error("decode page token: {0}")]
    Decode(String),
    /// The request's `page_size` was negative. AIP-158 lets the server pick a
    /// default for an absent (zero) size, but a negative size is nonsense, not a
    /// default request — so it is rejected rather than coerced.
    #[error("page_size must not be negative (got {requested})")]
    NegativePageSize {
        /// The negative `page_size` the client sent.
        requested: i32,
    },
}

/// A request carrying the AIP-158 pagination fields.
///
/// Reflection-free: implement these accessors for your request type (typically
/// trivial), or derive them. The reflective [`request_checksum`] is separate.
pub trait PageRequest {
    /// The opaque page token sent by the client (empty for the first page).
    fn page_token(&self) -> &str;
    /// The requested maximum page size.
    fn page_size(&self) -> i32;
    /// Optional AIP-158 skip; defaults to none.
    fn skip(&self) -> i32 {
        0
    }
}

/// One value in a cursor: a closed sum over the scalar shapes a sortable column
/// can hold. Null is not a cursor value — a sort over a nullable column would make
/// the seek ambiguous. Timestamps ride RFC3339 in [`Text`](CursorValue::Text) and
/// proto enums ride as their value name in `Text`.
///
/// Lives here, in the leaf crate, so it carries no dependency on the SQL layer; a
/// handler converts it to its store's bind value at the one boundary that depends
/// on both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CursorValue {
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// A 64-bit float.
    Double(f64),
    /// A UTF-8 string — also a timestamp (RFC3339) or an enum value name.
    Text(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

/// One cursor entry: a SQL column paired with the last row's value in it. The
/// cursor is self-describing — each entry names its column — so a token reads
/// legibly at the debug layer and its column list can be cross-checked against the
/// resolved `ORDER BY`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorEntry {
    /// The SQL column this value came from.
    pub column: String,
    /// The last row's value in that column.
    pub value: CursorValue,
}

/// A cursor page token: the last row's ordered key values, in `ORDER BY` clause
/// order, plus the request checksum that detects a request changed mid-pagination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageToken {
    /// The seek key: one entry per `ORDER BY` column, in clause order.
    pub cursor: Vec<CursorEntry>,
    /// Checksum of the request fields that must stay constant across pages.
    pub request_checksum: u32,
}

impl PageToken {
    /// Decode and verify a cursor page token from a request and the
    /// [`request_checksum`] of its non-pagination fields.
    ///
    /// An empty [`page_token`](PageRequest::page_token) is the first page, so it
    /// returns `Ok(None)`; a non-empty one is decoded (rejecting a stale-format
    /// token by its version byte) and its cursor returned. The token is rejected
    /// with [`Error::ChecksumMismatch`] when its recorded checksum disagrees with
    /// `checksum` — i.e. the client changed a non-pagination field (filter,
    /// order_by, parent, …) mid-pagination.
    ///
    /// The caller still cross-checks the cursor's columns against the resolved
    /// `ORDER BY` and each value's variant against its column type — that needs the
    /// schema, which this leaf crate does not depend on.
    pub fn parse(
        request: &impl PageRequest,
        checksum: u32,
    ) -> Result<Option<Vec<CursorEntry>>, Error> {
        if request.page_token().is_empty() {
            return Ok(None);
        }
        let token: Self = decode_page_token(request.page_token())?;
        if token.request_checksum != checksum {
            return Err(Error::ChecksumMismatch);
        }
        Ok(Some(token.cursor))
    }

    /// Mint a token from the last row's `cursor`, carrying `checksum` forward so
    /// the next page still rejects a changed request.
    pub fn encode(cursor: Vec<CursorEntry>, checksum: u32) -> String {
        encode_page_token(&Self {
            cursor,
            request_checksum: checksum,
        })
    }
}

/// The server's AIP-158 page-size policy: the size handed back for an unset
/// request, and the ceiling no single page may exceed.
///
/// Plain `Copy` struct, written as a literal at the call site (`SizeLimits {
/// default: 50, max: 1000 }`) — no constructor, because there is nothing to
/// validate that the caller cannot read off the fields. Both fields must be
/// positive; a non-positive `default` **or** `max` is a caller bug, not a
/// checked error, and yields a degenerate non-positive resolved size rather than
/// panicking (see [`Page::parse`]). The `default > max` case is the one
/// misconfiguration that *does* self-heal, via the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeLimits {
    /// Page size used when the request leaves `page_size` unset (zero). AIP-158
    /// lets the server pick this. Capped by [`max`](Self::max), so a misconfigured
    /// `default > max` self-heals to `max` rather than overshooting (assuming a
    /// positive `max`).
    pub default: i32,
    /// Upper bound on a single page, so a client cannot pull the whole result set
    /// in one request — AIP-158 lets the server return fewer than requested. Must
    /// be positive: it is the final cap on every resolved size, so a non-positive
    /// `max` is not corrected by anything downstream.
    pub max: i32,
}

/// The resolved AIP-158 pagination state for one list page: the decoded seek
/// cursor (the start, `None` on the first page) and the effective page size after
/// the policy default/cap.
///
/// Produced by [`Page::parse`], which folds the whole list-pagination preamble —
/// request checksum, token decode/verify, size resolution — into one call. A list
/// handler opens with `Page::parse(&req, limits)?`, seeks past
/// [`cursor`](Self::cursor) (a `Predicate::tuple_gt` in SQL, or a key comparison
/// over an in-memory listing), overfetches [`fetch_limit`](Self::fetch_limit)
/// rows, and hands the result to [`split_overfetch`](Self::split_overfetch) —
/// which truncates the probe row and mints the `next_page_token` from the last
/// kept row.
///
/// The fields are **private**: by [`parse`](Self::parse) the size is floored at
/// zero, so the width a handler reads is unsigned with no cast at the call site.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The decoded seek cursor, `None` on the first page.
    cursor: Option<Vec<CursorEntry>>,
    /// The page size after the [`SizeLimits`] default/cap, floored at zero by
    /// [`resolve_size`] so a degenerate [`SizeLimits`] yields a 0-size page rather
    /// than a wrapped cast (a negative *request* is the separate
    /// [`Error::NegativePageSize`]).
    size: i32,
    /// Carried into the next page's token so a changed request is still rejected.
    request_checksum: u32,
}

impl Page {
    /// Folds the AIP-158 list-pagination preamble into one step: checksum the
    /// request's non-pagination fields ([`request_checksum`]), decode and verify
    /// the cursor page token against that checksum ([`PageToken::parse`], which
    /// rejects a stale-format or changed-request token), and resolve the effective
    /// page size from `limits`.
    ///
    /// Size resolution (AIP-158): a negative `page_size` is rejected with
    /// [`Error::NegativePageSize`]; zero (unset) falls back to
    /// [`limits.default`](SizeLimits::default); the resulting size is then capped
    /// at [`limits.max`](SizeLimits::max) — the cap applies to **both** paths, so a
    /// misconfigured `default > max` self-heals to `max`. A non-positive
    /// `limits.default` is a documented caller bug, not a checked error: with an
    /// unset `page_size` it yields a degenerate zero-size page.
    pub fn parse<M: PageRequest + ReflectMessage>(
        request: &M,
        limits: SizeLimits,
    ) -> Result<Self, Error> {
        let checksum = request_checksum(request);
        let cursor = PageToken::parse(request, checksum)?;
        let size = resolve_size(request.page_size(), limits)?;
        Ok(Self {
            cursor,
            size,
            request_checksum: checksum,
        })
    }

    /// The decoded seek cursor — the last row of the previous page — or `None` on
    /// the first page. A handler turns it into the store's seek (a
    /// `Predicate::tuple_gt` over the ordered columns) after cross-checking its
    /// columns against the resolved `ORDER BY`.
    pub fn cursor(&self) -> Option<&[CursorEntry]> {
        self.cursor.as_deref()
    }

    /// The effective page size as an unsigned `u64` — the AIP-158 size after the
    /// [`SizeLimits`] default/cap, floored at zero by the internal size resolution.
    pub fn size(&self) -> u64 {
        // Floored at zero by `resolve_size`, so the cast cannot wrap.
        self.size as u64
    }

    /// The `LIMIT` for the overfetch probe: this page's [`size`](Self::size) plus
    /// one. Fetching one row past the page turns "is there another page?" into a
    /// length check the store answers for free — the extra row's *presence* is the
    /// `has_more` signal, and [`split_overfetch`](Self::split_overfetch) truncates
    /// it back off before the response. See the **Overfetch probe** glossary entry.
    pub fn fetch_limit(&self) -> u64 {
        // `size` is bounded by `SizeLimits.max` (an `i32`), so `+ 1` cannot overflow
        // a `u64`.
        self.size() + 1
    }

    /// Split a store-backed **overfetch** (a `LIMIT` of [`fetch_limit`](Self::fetch_limit)
    /// rows) into the page to return and the `next_page_token`: if the store handed
    /// back more rows than the page size, another page remains — truncate the extra
    /// probe row off and mint the token from the last kept row's cursor entries
    /// (`to_cursor`); otherwise the listing is exhausted and the token is empty.
    ///
    /// Pairs with [`fetch_limit`](Self::fetch_limit): the handler seeks past
    /// [`cursor`](Self::cursor), fetches `fetch_limit()` rows, then hands the result
    /// straight here. See the **Overfetch probe** glossary entry.
    pub fn split_overfetch<T, F>(&self, mut rows: Vec<T>, to_cursor: F) -> (Vec<T>, String)
    where
        F: FnOnce(&T) -> Vec<CursorEntry>,
    {
        // The probe row makes the result longer than the page exactly when a
        // further page exists.
        let has_more = rows.len() as u64 > self.size();
        // Truncate is a no-op when the store returned a short final page.
        rows.truncate(usize::try_from(self.size()).unwrap_or(usize::MAX));
        // Mint the next token from the last kept row's key, or end the listing.
        let token = match (has_more, rows.last()) {
            (true, Some(last)) => PageToken::encode(to_cursor(last), self.request_checksum),
            _ => String::new(),
        };
        (rows, token)
    }
}

/// Resolves the effective page size from a request's `page_size` per AIP-158: a
/// negative value is [`Error::NegativePageSize`], zero/unset falls back to
/// `limits.default`, and the result is capped at `limits.max` (the cap applies to
/// the default too, so `default > max` self-heals to `max`) and floored at zero.
fn resolve_size(requested: i32, limits: SizeLimits) -> Result<i32, Error> {
    if requested < 0 {
        return Err(Error::NegativePageSize { requested });
    }
    // Zero means "unset" — take the server default; the cap then applies to
    // whichever value we landed on.
    let base = if requested == 0 {
        limits.default
    } else {
        requested
    };
    // Floor at zero so a degenerate [`SizeLimits`] (a non-positive `max`, a caller
    // bug per its docs) yields a harmless 0-size page rather than a negative size
    // that [`Page::size`]'s `as u64` cast would balloon into billions of rows.
    Ok(base.min(limits.max).max(0))
}

/// Encodes an arbitrary cursor payload as an opaque page token.
///
/// The payload is `postcard`-serialized, tagged with the 1-byte version
/// prefix, and base64url-encoded. The offset [`PageToken`] is just one such
/// payload.
pub fn encode_page_token<T: Serialize>(value: &T) -> String {
    // Page-token payloads are small, owned structs whose `Serialize` impls do
    // not fail; a serialization error here would be a bug, not bad input, so we
    // surface it loudly rather than minting a corrupt token.
    let payload = postcard::to_allocvec(value)
        .expect("postcard serialization of a page-token payload does not fail");
    let mut bytes = Vec::with_capacity(payload.len() + 1);
    bytes.push(PAGE_TOKEN_VERSION);
    bytes.extend_from_slice(&payload);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes an opaque page token into a cursor payload.
///
/// Rejects a token whose version prefix is absent ([`Error::Malformed`]) or
/// unrecognized ([`Error::UnsupportedVersion`]) instead of mis-decoding it
/// under the current format.
pub fn decode_page_token<T: serde::de::DeserializeOwned>(token: &str) -> Result<T, Error> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|e| Error::Decode(e.to_string()))?;
    let (&version, payload) = bytes.split_first().ok_or(Error::Malformed)?;
    if version != PAGE_TOKEN_VERSION {
        return Err(Error::UnsupportedVersion {
            found: version,
            expected: PAGE_TOKEN_VERSION,
        });
    }
    postcard::from_bytes(payload).map_err(|e| Error::Decode(e.to_string()))
}

/// Computes the CRC32-IEEE checksum of a request, excluding the pagination
/// fields (`page_token`, `page_size`, `skip`).
///
/// The headline reflective surface (ADR-0009): generic over [`ReflectMessage`],
/// so the request's [`MessageDescriptor`](prost_reflect::MessageDescriptor)
/// travels with the value and no descriptor pool is threaded through the call.
/// Unlike the field-mask primitive this is a single generic function, not a
/// facade/core pair — it only *reads* the request, so it needs no `Default` and
/// no decode-back. Because `prost_reflect::DynamicMessage` itself implements
/// [`ReflectMessage`], a `&DynamicMessage` caller (the crate's tests, a
/// JSON/gateway path) keeps compiling unchanged.
///
/// Returns a bare `u32`: the checksum can never *fail* — the only thing that
/// could go wrong is the descriptor-round-trip below, which is a build invariant,
/// not bad input. So there is no error to thread back, and [`Page::parse`] calls
/// this without a `?`.
///
/// Ported from `aip-go`'s `CalculateRequestChecksum`: the request is transcoded
/// to a [`DynamicMessage`] (the pagination fields are cleared by name, which only
/// a dynamic message can do), those fields that legitimately change between pages
/// are cleared, and the prost-encoded remainder is checksummed. Any *other* field
/// changing flips the checksum, which is how [`PageToken::parse`] detects a
/// request that mutated mid-pagination. A request that does not declare a given
/// pagination field (e.g. one without `skip`) simply has nothing to clear for it.
pub fn request_checksum<M: ReflectMessage>(request: &M) -> u32 {
    let descriptor = request.descriptor();
    // Transcode through wire bytes into a dynamic message so the pagination
    // fields can be cleared by name. The round-trip can only fail if a message
    // and its descriptor disagree — a build/config bug, not bad input — so it is
    // treated as an invariant rather than an error variant (ADR-0009).
    let mut cloned = DynamicMessage::decode(descriptor.clone(), request.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    for name in ["page_token", "page_size", "skip"] {
        if let Some(field) = descriptor.get_field_by_name(name) {
            cloned.clear_field(&field);
        }
    }
    crc32fast::hash(&cloned.encode_to_vec())
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `reason` + `domain` (`aip-rs`) and the
    /// error's dynamic values as `metadata`. A page token is an opaque value
    /// rather than a request field path, so the token variants attach no
    /// `BadRequest`; [`NegativePageSize`](Error::NegativePageSize) is the lone
    /// exception — it names the `page_size` request field, so it carries a
    /// `BadRequest` field violation alongside the `ErrorInfo`.
    /// See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let mut details = ErrorDetails::new();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::Malformed => ("PAGE_TOKEN_MALFORMED", HashMap::new()),
            Error::UnsupportedVersion { found, expected } => (
                "PAGE_TOKEN_UNSUPPORTED_VERSION",
                HashMap::from([
                    ("found".to_owned(), found.to_string()),
                    ("expected".to_owned(), expected.to_string()),
                ]),
            ),
            Error::ChecksumMismatch => ("PAGE_TOKEN_CHECKSUM_MISMATCH", HashMap::new()),
            Error::Decode(detail) => (
                "PAGE_TOKEN_DECODE",
                HashMap::from([("detail".to_owned(), detail.clone())]),
            ),
            Error::NegativePageSize { requested } => {
                // Unlike a page token, `page_size` is a named request field, so
                // the validation failure points at it with a `BadRequest`
                // (ADR-0007), the first variant in this crate to carry one.
                details.add_bad_request_violation("page_size", &message);
                (
                    "PAGE_SIZE_NEGATIVE",
                    HashMap::from([("page_size".to_owned(), requested.to_string())]),
                )
            }
        };
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn unsupported_version_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::UnsupportedVersion {
            found: 9,
            expected: PAGE_TOKEN_VERSION,
        }
        .into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "PAGE_TOKEN_UNSUPPORTED_VERSION");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(info.metadata.get("found").map(String::as_str), Some("9"));

        // A page token is an opaque value, not a request field path.
        assert!(status.get_details_bad_request().is_none());
    }

    #[test]
    fn negative_page_size_maps_to_invalid_argument_with_bad_request() {
        let status: tonic::Status = Error::NegativePageSize { requested: -3 }.into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "PAGE_SIZE_NEGATIVE");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("page_size").map(String::as_str),
            Some("-3"),
        );

        // Unlike a page token, `page_size` is a named request field, so the
        // violation points at it (ADR-0007).
        let bad = status
            .get_details_bad_request()
            .expect("a BadRequest field violation is attached for a named field");
        assert_eq!(bad.field_violations[0].field, "page_size");
    }
}

#[cfg(test)]
mod tests {
    // `super::*` brings the `base64::Engine` trait into scope for the
    // `URL_SAFE_NO_PAD.encode`/`.decode` calls in the version-prefix test.
    use super::*;

    /// An arbitrary cursor payload — a key-based token need not be the offset
    /// [`PageToken`]; any `Serialize`/`DeserializeOwned` type round-trips.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Cursor {
        last_name: String,
        count: u32,
    }

    #[test]
    fn round_trips_arbitrary_payloads() {
        // Mirrors aip-go's `Test_PageTokenStruct`: encode then decode is the
        // identity, including for zero/default field values.
        for cursor in [
            Cursor {
                last_name: "shippers/42".to_owned(),
                count: 7,
            },
            Cursor {
                last_name: String::new(),
                count: 0,
            },
        ] {
            let token = encode_page_token(&cursor);
            let decoded: Cursor = decode_page_token(&token).expect("round-trips");
            assert_eq!(decoded, cursor);
        }
    }

    /// A cursor entry over `column` holding `value`.
    fn entry(column: &str, value: CursorValue) -> CursorEntry {
        CursorEntry {
            column: column.to_owned(),
            value,
        }
    }

    #[test]
    fn round_trips_cursor_page_token() {
        // Every cursor-value variant survives the postcard + base64url round-trip.
        let token = PageToken {
            cursor: vec![
                entry("display_name", CursorValue::Text("Oslo Dock".to_owned())),
                entry("size", CursorValue::Int(-3)),
                entry("ratio", CursorValue::Double(1.5)),
                entry("active", CursorValue::Bool(true)),
                entry("blob", CursorValue::Bytes(vec![1, 2, 3])),
            ],
            request_checksum: 0xDEAD_BEEF,
        };
        let encoded = PageToken::encode(token.cursor.clone(), token.request_checksum);
        let decoded: PageToken = decode_page_token(&encoded).expect("round-trips");
        assert_eq!(decoded, token);
    }

    #[test]
    fn wrong_version_prefix_is_rejected() {
        let encoded = PageToken::encode(
            vec![entry("shipper", CursorValue::Text("acme".to_owned()))],
            2,
        );
        // Flip the version byte (the first decoded byte) and re-encode, leaving
        // the rest of the payload intact.
        let mut bytes = URL_SAFE_NO_PAD.decode(&encoded).expect("valid base64url");
        bytes[0] = PAGE_TOKEN_VERSION.wrapping_add(1);
        let tampered = URL_SAFE_NO_PAD.encode(bytes);
        let err = decode_page_token::<PageToken>(&tampered).expect_err("wrong version");
        assert!(
            matches!(err, Error::UnsupportedVersion { found, expected }
                if found == PAGE_TOKEN_VERSION.wrapping_add(1) && expected == PAGE_TOKEN_VERSION),
            "{err:?}",
        );
    }

    #[test]
    fn absent_prefix_is_rejected() {
        // The empty token carries no version byte at all.
        let err = decode_page_token::<PageToken>("").expect_err("no version byte");
        assert!(matches!(err, Error::Malformed), "{err:?}");
    }

    #[test]
    fn malformed_base64_is_rejected() {
        // '*' is outside the base64url alphabet.
        let err = decode_page_token::<PageToken>("not*base64").expect_err("bad base64");
        assert!(matches!(err, Error::Decode(_)), "{err:?}");
    }

    #[test]
    fn page_request_is_implementable_without_reflection() {
        // A plain request struct satisfies `PageRequest` with trivial accessors
        // and no protobuf/reflection machinery.
        struct ListReq {
            page_token: String,
            page_size: i32,
        }
        impl PageRequest for ListReq {
            fn page_token(&self) -> &str {
                &self.page_token
            }
            fn page_size(&self) -> i32 {
                self.page_size
            }
        }
        let req = ListReq {
            page_token: "abc".to_owned(),
            page_size: 25,
        };
        assert_eq!(req.page_token(), "abc");
        assert_eq!(req.page_size(), 25);
        assert_eq!(req.skip(), 0); // default
    }

    /// A reflection-free request used to exercise [`PageToken::parse`];
    /// `page_size` is irrelevant to parsing, so it is fixed at 0.
    struct CursorReq {
        page_token: String,
    }
    impl PageRequest for CursorReq {
        fn page_token(&self) -> &str {
            &self.page_token
        }
        fn page_size(&self) -> i32 {
            0
        }
    }

    #[test]
    fn parse_empty_token_is_the_first_page() {
        // No token → no cursor; the listing starts from the top.
        let cursor = PageToken::parse(
            &CursorReq {
                page_token: String::new(),
            },
            0xABCD,
        )
        .expect("empty token parses");
        assert_eq!(cursor, None);
    }

    #[test]
    fn parse_valid_token_returns_the_cursor() {
        let checksum = 0x1234_5678;
        let seek = vec![
            entry("display_name", CursorValue::Text("Oslo Dock".to_owned())),
            entry("site", CursorValue::Text("dock-1".to_owned())),
        ];
        let parsed = PageToken::parse(
            &CursorReq {
                page_token: PageToken::encode(seek.clone(), checksum),
            },
            checksum,
        )
        .expect("matching checksum parses");
        assert_eq!(parsed, Some(seek));
    }

    #[test]
    fn parse_rejects_checksum_mismatch() {
        // A token minted against one request is rejected when replayed against a
        // request whose non-pagination fields changed (different checksum).
        let err = PageToken::parse(
            &CursorReq {
                page_token: PageToken::encode(
                    vec![entry("site", CursorValue::Text("dock-1".to_owned()))],
                    0x1111,
                ),
            },
            0x2222,
        )
        .expect_err("checksum mismatch");
        assert!(matches!(err, Error::ChecksumMismatch), "{err:?}");
    }

    #[test]
    fn parse_propagates_decode_errors() {
        // A malformed (non-empty) token surfaces the decode error rather than
        // being mistaken for the start of the result set.
        let err = PageToken::parse(
            &CursorReq {
                page_token: "not*base64".to_owned(),
            },
            0,
        )
        .expect_err("malformed token");
        assert!(matches!(err, Error::Decode(_)), "{err:?}");
    }

    /// Builds a `ListSitesRequest` fixture (it carries `parent`, `page_size`,
    /// `page_token`, and `skip` — the full pagination field set) from JSON.
    fn list_sites(json: &str) -> DynamicMessage {
        test_fixtures::from_json("einride.example.freight.v1.ListSitesRequest", json)
            .expect("ListSitesRequest fixture builds")
    }

    #[test]
    fn request_checksum_ignores_pagination_fields() {
        // Two requests that differ only in their pagination fields must share a
        // checksum — that is exactly what changes legitimately between pages.
        let a =
            list_sites(r#"{"parent":"shippers/acme","pageSize":10,"pageToken":"first","skip":5}"#);
        let b = list_sites(
            r#"{"parent":"shippers/acme","pageSize":99,"pageToken":"second","skip":40}"#,
        );
        assert_eq!(request_checksum(&a), request_checksum(&b));
    }

    #[test]
    fn request_checksum_changes_when_other_field_changes() {
        // A change to any non-pagination field (here `parent`) must flip the
        // checksum, so a stale token is rejected.
        let a = list_sites(r#"{"parent":"shippers/acme"}"#);
        let b = list_sites(r#"{"parent":"shippers/other"}"#);
        assert_ne!(request_checksum(&a), request_checksum(&b));
    }

    #[test]
    fn request_checksum_handles_request_without_skip() {
        // `ListShippersRequest` declares only `page_size`/`page_token` (no
        // `skip`/`parent`): clearing the absent `skip` is a no-op, and two
        // pagination-only-different requests still match.
        let a = test_fixtures::from_json(
            "einride.example.freight.v1.ListShippersRequest",
            r#"{"pageSize":10,"pageToken":"first"}"#,
        )
        .expect("ListShippersRequest fixture builds");
        let b = test_fixtures::from_json(
            "einride.example.freight.v1.ListShippersRequest",
            r#"{"pageSize":20,"pageToken":"second"}"#,
        )
        .expect("ListShippersRequest fixture builds");
        assert_eq!(request_checksum(&a), request_checksum(&b));
    }

    #[test]
    fn resolve_size_applies_aip158_rules() {
        // AIP-158 size resolution, migrated from freight-server's
        // `effective_page_size_applies_aip158_rules`: a negative `page_size` is
        // rejected, zero/unset falls back to the default, a positive value passes
        // through, and anything above the cap is clamped to the max.
        let limits = SizeLimits {
            default: 50,
            max: 1000,
        };
        let err = resolve_size(-1, limits).expect_err("negative is rejected");
        assert!(
            matches!(err, Error::NegativePageSize { requested } if requested == -1),
            "{err:?}",
        );
        assert_eq!(resolve_size(0, limits).expect("zero is the default"), 50);
        assert_eq!(
            resolve_size(10, limits).expect("positive passes through"),
            10
        );
        assert_eq!(
            resolve_size(i32::MAX, limits).expect("over-max is clamped"),
            1000,
        );
    }

    #[test]
    fn resolve_size_caps_a_misconfigured_default() {
        // The cap applies to the default path too: a `default > max` self-heals to
        // `max` rather than overshooting when the request leaves `page_size` unset.
        let limits = SizeLimits {
            default: 5000,
            max: 1000,
        };
        assert_eq!(
            resolve_size(0, limits).expect("the over-cap default is itself capped"),
            1000,
        );
    }

    #[test]
    fn resolve_size_floors_a_degenerate_size_limit_at_zero() {
        // A non-positive `max` is a documented caller bug, not a checked error. The
        // resolved size floors at 0 (a loud, harmless empty page) rather than going
        // negative — which `Page::size`'s `as u64` cast would otherwise balloon into
        // billions of rows. Both an unset and a positive request size land there.
        let degenerate = SizeLimits {
            default: 50,
            max: 0,
        };
        assert_eq!(resolve_size(0, degenerate).expect("unset → floored 0"), 0);
        assert_eq!(resolve_size(10, degenerate).expect("capped to 0"), 0);

        // A negative `max` caps below zero, then the floor lifts it back to 0.
        let negative_max = SizeLimits {
            default: 50,
            max: -5,
        };
        assert_eq!(resolve_size(10, negative_max).expect("floored 0"), 0);
    }

    // `Page::parse` folds `request_checksum` + `PageToken::parse` + `resolve_size`,
    // each unit-tested above. The fold needs a `PageRequest + ReflectMessage`
    // request — a generated type, which the crate's `DynamicMessage` fixtures are
    // not (`PageRequest` returns `&str`, which a reflective field read cannot
    // yield) — so its end-to-end coverage (empty token, negative size, stale-token
    // guard) lives in freight-server's `list_*` handler tests, where the generated
    // requests carry both impls.

    /// Builds a [`Page`] with page size `size` and no seek cursor — the
    /// post-validation state, constructed directly (private fields are in reach
    /// crate-internally) so the overfetch helper can be unit-tested without a
    /// generated `PageRequest + ReflectMessage` request.
    fn page(size: i32) -> Page {
        Page {
            cursor: None,
            size,
            request_checksum: 0x1234,
        }
    }

    /// A u32 row's cursor: a single `n` column holding the row as an integer.
    fn to_cursor(n: &u32) -> Vec<CursorEntry> {
        vec![entry("n", CursorValue::Int(i64::from(*n)))]
    }

    /// Decodes a non-empty `next_page_token` back to its cursor, asserting it is
    /// non-empty first — the shape every "more pages remain" assertion below checks.
    fn next_cursor(token: &str) -> Vec<CursorEntry> {
        let decoded: PageToken = decode_page_token(token).expect("a non-empty token round-trips");
        decoded.cursor
    }

    #[test]
    fn cursor_accessor_returns_the_decoded_seek() {
        // The seek a handler turns into a `tuple_gt`; `None` on the first page.
        let seek = vec![entry("shipper", CursorValue::Text("acme".to_owned()))];
        let seeking = Page {
            cursor: Some(seek.clone()),
            size: 10,
            request_checksum: 0,
        };
        assert_eq!(seeking.cursor(), Some(seek.as_slice()));
        assert!(page(10).cursor().is_none());
    }

    #[test]
    fn size_accessor_is_unsigned() {
        // The post-validation surface: a floored size reads back as plain `u64`.
        assert_eq!(page(10).size(), 10);
    }

    #[test]
    fn fetch_limit_is_size_plus_one() {
        // The overfetch probe pulls one row past the page so its presence answers
        // `has_more`.
        assert_eq!(page(10).fetch_limit(), 11);
        assert_eq!(page(0).fetch_limit(), 1);
    }

    #[test]
    fn split_overfetch_overfull_truncates_and_mints_token() {
        // The store handed back `size + 1` rows: the probe row proves a further page
        // exists. Truncate it off, mint the token from the last kept row's key.
        let (rows, token) = page(2).split_overfetch(vec![10, 20, 30], to_cursor);
        assert_eq!(rows, [10, 20]);
        assert_eq!(next_cursor(&token), vec![entry("n", CursorValue::Int(20))],);
    }

    #[test]
    fn split_overfetch_exact_fit_has_no_next_token() {
        // Exactly `size` rows came back ⇒ no probe row ⇒ the listing is exhausted.
        let (rows, token) = page(2).split_overfetch(vec![10, 20], to_cursor);
        assert_eq!(rows, [10, 20]);
        assert_eq!(token, "");
    }

    #[test]
    fn split_overfetch_under_filled_page_has_no_next_token() {
        // A short final page (fewer than `size` rows) ⇒ exhausted.
        let (rows, token) = page(2).split_overfetch(vec![50], to_cursor);
        assert_eq!(rows, [50]);
        assert_eq!(token, "");
    }

    #[test]
    fn split_overfetch_empty_store_yields_empty_page() {
        // The store returned nothing ⇒ an empty page and no token.
        let (rows, token) = page(2).split_overfetch(Vec::<u32>::new(), to_cursor);
        assert!(rows.is_empty());
        assert_eq!(token, "");
    }
}
