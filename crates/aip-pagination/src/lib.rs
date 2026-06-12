//! AIP-158 pagination: page tokens and request checksums.
//!
//! Page tokens are encoded with `serde` + `postcard` + base64url (a 1-byte
//! version prefix guards against format drift) and are deliberately *not*
//! wire-compatible with `aip-go`'s gob tokens.
//!
//! See <https://google.aip.dev/158>.
#![cfg_attr(docsrs, feature(doc_cfg))]

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use prost::Message as _;
use prost_reflect::{DynamicMessage, ReflectMessage};
use serde::{Deserialize, Serialize};

/// Version byte prepended to every encoded page token. Bump it whenever the
/// token wire format changes so that tokens minted by an older format fail
/// loudly (see ADR-0004) instead of silently mis-decoding under the new one.
const PAGE_TOKEN_VERSION: u8 = 1;

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

/// An offset-based page token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageToken {
    /// Offset of the page into the result set.
    pub offset: i64,
    /// Checksum of the request fields that must stay constant across pages.
    pub request_checksum: u32,
}

impl PageToken {
    /// Parses and verifies an offset page token from a request and the
    /// [`request_checksum`] of its non-pagination fields.
    ///
    /// An empty [`page_token`](PageRequest::page_token) yields offset 0 (plus any
    /// [`skip`](PageRequest::skip)); a non-empty one is decoded and its stored
    /// offset returned (again with `skip` applied on top, per AIP-158). The token
    /// is rejected with [`Error::ChecksumMismatch`] when its recorded checksum
    /// disagrees with `checksum` — i.e. the client changed a non-pagination field
    /// (filter, order_by, parent, …) mid-pagination.
    ///
    /// Unlike `aip-go`, no `pageTokenChecksumMask` is applied: the 1-byte version
    /// prefix already forces older tokens to fail loudly across a format change
    /// (see ADR-0004), so the mask would be redundant.
    pub fn parse(request: &impl PageRequest, checksum: u32) -> Result<Self, Error> {
        let skip = i64::from(request.skip());
        if request.page_token().is_empty() {
            return Ok(Self {
                offset: skip,
                request_checksum: checksum,
            });
        }
        let token: Self = decode_page_token(request.page_token())?;
        if token.request_checksum != checksum {
            return Err(Error::ChecksumMismatch);
        }
        Ok(Self {
            // Tokens are unsigned and therefore client-forgeable (ADR-0004), so
            // saturate rather than risk an overflow panic on a hostile offset.
            offset: token.offset.saturating_add(skip),
            request_checksum: token.request_checksum,
        })
    }

    /// The token for the next page, given the current page size.
    pub fn next(self, page_size: i32) -> Self {
        Self {
            offset: self.offset + i64::from(page_size),
            ..self
        }
    }

    /// Encode this token to its opaque string form.
    pub fn encode(&self) -> String {
        encode_page_token(self)
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

/// The resolved AIP-158 pagination state for one list page: the verified offset
/// [`PageToken`] and the effective page size after the policy default/cap has
/// been applied.
///
/// Produced by [`Page::parse`], which folds the whole list-pagination preamble —
/// request checksum, token parse/verify, size resolution — into one call. A list
/// handler opens with `Page::parse(&req, limits)?`, reads the page start and width
/// through the unsigned [`offset`](Self::offset) / [`size`](Self::size) accessors,
/// and **owns applying itself to the results**: [`apply`](Self::apply) for a
/// collection already in memory, or the [`fetch_limit`](Self::fetch_limit) /
/// [`split_overfetch`](Self::split_overfetch) pair for a store-backed listing.
/// Each of those mints the `next_page_token` the handler returns.
///
/// The fields are **private**: by [`parse`](Self::parse) the offset is clamped
/// non-negative and the size is floored at zero, so the post-validation surface a
/// handler reads is unsigned — there is no signed [`PageToken::offset`] or `i32`
/// size left to cast at the call site. The raw signed `i64`/`i32` representation is
/// an internal detail, not the consumer story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// The verified offset page token, its `offset` clamped non-negative by
    /// [`parse`](Self::parse) — where this page starts.
    token: PageToken,
    /// The page size after the [`SizeLimits`] default/cap has been applied, floored
    /// at zero by [`resolve_size`] so a degenerate [`SizeLimits`] yields a 0-size
    /// page rather than a wrapped cast (a negative *request* is the separate
    /// [`Error::NegativePageSize`]).
    size: i32,
}

impl Page {
    /// Folds the AIP-158 list-pagination preamble into one step: checksum the
    /// request's non-pagination fields ([`request_checksum`]), parse and verify
    /// the offset page token against that checksum ([`PageToken::parse`], which
    /// rejects a request that changed mid-pagination), and resolve the effective
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
        let mut token = PageToken::parse(request, checksum)?;
        // Clamp a forged negative offset to 0 *here*, not in the accessor: the
        // next-page token is minted from `offset + size` (see [`PageToken::next`]),
        // so clamping only on read would advance from `negative + size` and skip
        // rows. Tokens are unsigned and client-forgeable (ADR-0004), so this is the
        // one place the negative is neutralized for both the served page and its
        // successor. `PageToken::offset` stays a signed wire artifact.
        token.offset = token.offset.max(0);
        let size = resolve_size(request.page_size(), limits)?;
        Ok(Self { token, size })
    }

    /// Where this page starts: the verified offset, clamped non-negative by
    /// [`parse`](Self::parse), as an unsigned `u64`.
    ///
    /// This is the post-validation surface a list handler hands to its store
    /// (`OFFSET ?`) — no `as u64` / `try_from` ceremony at the call site, because
    /// the clamp already happened.
    pub fn offset(&self) -> u64 {
        // Clamped non-negative at parse, so the cast cannot wrap into the high
        // `u64` range.
        self.token.offset as u64
    }

    /// The effective page size as an unsigned `u64` — the AIP-158 size after the
    /// [`SizeLimits`] default/cap, floored at zero by [`resolve_size`].
    ///
    /// The unsigned partner to [`offset`](Self::offset): the width a handler passes
    /// to a store or compares a result length against, cast-free.
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

    /// Apply this page to a collection that **lives in memory**: slice out the
    /// page's window, decide whether more remain, and mint the
    /// `next_page_token` — returning the page rows paired with that token.
    ///
    /// For the freight demo this is the shipper `BTreeMap` listing and any
    /// post-filter set (e.g. the soft-delete visibility filter) — collections the
    /// handler already holds in full. The `Vec` is consumed and the page drained
    /// out of it in place, so there are no element clones.
    ///
    /// **Do not** `fetch_all().apply(..)` a store-backed listing to use this: that
    /// pulls the whole table into memory to serve one page, exactly what the
    /// [`fetch_limit`](Self::fetch_limit) / [`split_overfetch`](Self::split_overfetch)
    /// pair exists to avoid. This helper is for collections that are *already*
    /// resident, not a shortcut around paging in the store.
    pub fn apply<T>(&self, mut items: Vec<T>) -> (Vec<T>, String) {
        let total = items.len();
        // `offset`/`size` are bounded unsigned values; saturate the `usize`
        // conversions so a 32-bit target degrades to "past the end" (an empty page)
        // rather than wrapping. `.min(total)` keeps both within the collection.
        let start = usize::try_from(self.offset())
            .unwrap_or(usize::MAX)
            .min(total);
        let width = usize::try_from(self.size()).unwrap_or(usize::MAX);
        let end = start.saturating_add(width).min(total);
        // More remain only when this page stops short of the full collection.
        let token = self.next_token(end < total);
        // Drop the tail past `end`, then drain the `0..start` prefix away (the
        // `Drain` drop shifts the rest down), leaving exactly `start..end` — no
        // clones.
        items.truncate(end);
        items.drain(..start);
        (items, token)
    }

    /// Split a store-backed **overfetch** (a `LIMIT` of [`fetch_limit`](Self::fetch_limit)
    /// rows) into the page to return and the `next_page_token`: if the store handed
    /// back more rows than the page size, another page remains — truncate the extra
    /// probe row off and mint the token; otherwise the listing is exhausted.
    ///
    /// Pairs with [`fetch_limit`](Self::fetch_limit): the handler fetches
    /// `fetch_limit()` rows at [`offset`](Self::offset), then hands the result
    /// straight here. See the **Overfetch probe** glossary entry.
    pub fn split_overfetch<T>(&self, mut rows: Vec<T>) -> (Vec<T>, String) {
        // The probe row makes the result longer than the page exactly when a
        // further page exists.
        let has_more = rows.len() as u64 > self.size();
        // Truncate is a no-op when the store returned a short final page.
        rows.truncate(usize::try_from(self.size()).unwrap_or(usize::MAX));
        let token = self.next_token(has_more);
        (rows, token)
    }

    /// The opaque token for the page after this one, or the empty string when no
    /// further page remains.
    ///
    /// The escape hatch behind [`apply`](Self::apply) and
    /// [`split_overfetch`](Self::split_overfetch) — reach for it directly only in a
    /// custom flow they do not cover (e.g. a total-count handler computing
    /// `has_more` as `end < total` itself).
    ///
    /// `has_more` is the caller's "is there another page?" signal. When set, the
    /// token advances the offset by this page's [`size`](Self::size), carrying the
    /// request checksum forward so the next page still rejects a changed request;
    /// when unset, the empty string tells the client the listing is exhausted.
    pub fn next_token(&self, has_more: bool) -> String {
        if has_more {
            self.token.next(self.size).encode()
        } else {
            String::new()
        }
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
    /// carrying a machine-readable `reason` + [`domain`](ERROR_DOMAIN) and the
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

    #[test]
    fn round_trips_offset_page_token() {
        let token = PageToken {
            offset: 100,
            request_checksum: 0xDEAD_BEEF,
        };
        let decoded: PageToken = decode_page_token(&token.encode()).expect("round-trips");
        assert_eq!(decoded, token);
    }

    #[test]
    fn next_advances_offset_by_page_size() {
        let token = PageToken {
            offset: 10,
            request_checksum: 0x1234,
        };
        let next = token.next(20);
        assert_eq!(next.offset, 30);
        // The checksum is carried through unchanged across pages.
        assert_eq!(next.request_checksum, token.request_checksum);
    }

    #[test]
    fn wrong_version_prefix_is_rejected() {
        let encoded = PageToken {
            offset: 1,
            request_checksum: 2,
        }
        .encode();
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

    /// A reflection-free offset request used to exercise [`PageToken::parse`];
    /// `page_size` is irrelevant to parsing, so it is fixed at 0.
    struct OffsetReq {
        page_token: String,
        skip: i32,
    }
    impl PageRequest for OffsetReq {
        fn page_token(&self) -> &str {
            &self.page_token
        }
        fn page_size(&self) -> i32 {
            0
        }
        fn skip(&self) -> i32 {
            self.skip
        }
    }

    #[test]
    fn parse_empty_token_starts_at_skip() {
        // No token → offset 0, carrying the supplied checksum forward so the next
        // page can detect a changed request.
        let first = PageToken::parse(
            &OffsetReq {
                page_token: String::new(),
                skip: 0,
            },
            0xABCD,
        )
        .expect("empty token parses");
        assert_eq!(first.offset, 0);
        assert_eq!(first.request_checksum, 0xABCD);

        // Skip shifts the very first page (AIP-158).
        let skipped = PageToken::parse(
            &OffsetReq {
                page_token: String::new(),
                skip: 5,
            },
            0xABCD,
        )
        .expect("empty token with skip parses");
        assert_eq!(skipped.offset, 5);
    }

    #[test]
    fn parse_valid_token_returns_stored_offset() {
        let checksum = 0x1234_5678;
        let minted = PageToken {
            offset: 100,
            request_checksum: checksum,
        };
        let parsed = PageToken::parse(
            &OffsetReq {
                page_token: minted.encode(),
                skip: 0,
            },
            checksum,
        )
        .expect("matching checksum parses");
        assert_eq!(parsed.offset, 100);

        // Skip stacks on top of the token's recorded position.
        let skipped = PageToken::parse(
            &OffsetReq {
                page_token: minted.encode(),
                skip: 5,
            },
            checksum,
        )
        .expect("matching checksum parses");
        assert_eq!(skipped.offset, 105);
    }

    #[test]
    fn parse_rejects_checksum_mismatch() {
        // A token minted against one request is rejected when replayed against a
        // request whose non-pagination fields changed (different checksum).
        let minted = PageToken {
            offset: 100,
            request_checksum: 0x1111,
        };
        let err = PageToken::parse(
            &OffsetReq {
                page_token: minted.encode(),
                skip: 0,
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
            &OffsetReq {
                page_token: "not*base64".to_owned(),
                skip: 0,
            },
            0,
        )
        .expect_err("malformed token");
        assert!(matches!(err, Error::Decode(_)), "{err:?}");
    }

    #[test]
    fn parse_saturates_offset_on_overflow() {
        // Tokens are unsigned and forgeable: a near-max offset plus a skip must
        // saturate rather than overflow-panic (a debug-build crash otherwise).
        let minted = PageToken {
            offset: i64::MAX,
            request_checksum: 0,
        };
        let parsed = PageToken::parse(
            &OffsetReq {
                page_token: minted.encode(),
                skip: 5,
            },
            0,
        )
        .expect("forged near-max offset parses");
        assert_eq!(parsed.offset, i64::MAX);
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

    #[test]
    fn next_token_is_empty_at_the_end_and_advances_otherwise() {
        // `next_token(false)` ends the listing with the empty string; `(true)`
        // mints the follow-on token, advancing the offset by the page size and
        // carrying the checksum forward.
        let page = Page {
            token: PageToken {
                offset: 20,
                request_checksum: 0x1234,
            },
            size: 10,
        };
        assert_eq!(page.next_token(false), "");

        let next = page.next_token(true);
        assert!(!next.is_empty());
        let decoded: PageToken = decode_page_token(&next).expect("the next token round-trips");
        assert_eq!(decoded.offset, 30);
        assert_eq!(decoded.request_checksum, 0x1234);
    }

    /// Builds a [`Page`] at `offset` with page size `size` — the unsigned accessors'
    /// post-validation state, constructed directly (private fields are in reach
    /// crate-internally) so the apply/overfetch helpers can be unit-tested without a
    /// generated `PageRequest + ReflectMessage` request.
    fn page(offset: i64, size: i32) -> Page {
        Page {
            token: PageToken {
                offset,
                request_checksum: 0x1234,
            },
            size,
        }
    }

    /// Decodes a non-empty `next_page_token` back to its offset, asserting it is
    /// non-empty first — the shape every "more pages remain" assertion below checks.
    fn next_offset(token: &str) -> i64 {
        let decoded: PageToken = decode_page_token(token).expect("a non-empty token round-trips");
        decoded.offset
    }

    #[test]
    fn offset_and_size_accessors_are_unsigned() {
        // The post-validation surface: a clamped offset and a floored size read back
        // as plain `u64`, no cast at the call site.
        let page = page(20, 10);
        assert_eq!(page.offset(), 20);
        assert_eq!(page.size(), 10);
    }

    #[test]
    fn fetch_limit_is_size_plus_one() {
        // The overfetch probe pulls one row past the page so its presence answers
        // `has_more`.
        assert_eq!(page(0, 10).fetch_limit(), 11);
        assert_eq!(page(40, 0).fetch_limit(), 1);
    }

    #[test]
    fn apply_empty_collection_yields_empty_page_and_no_token() {
        // Nothing in memory ⇒ an empty page and an exhausted listing.
        let (items, token) = page(0, 10).apply(Vec::<u32>::new());
        assert!(items.is_empty());
        assert_eq!(token, "");
    }

    #[test]
    fn apply_full_page_mid_listing_mints_next_token() {
        // A full page with rows beyond it: the window is sliced out and the token
        // advances the offset by the page size.
        let (items, token) = page(0, 2).apply(vec![10, 20, 30, 40, 50]);
        assert_eq!(items, [10, 20]);
        assert_eq!(next_offset(&token), 2);

        // The second page starts where the first stopped.
        let (items, token) = page(2, 2).apply(vec![10, 20, 30, 40, 50]);
        assert_eq!(items, [30, 40]);
        assert_eq!(next_offset(&token), 4);
    }

    #[test]
    fn apply_exact_fit_last_page_has_no_next_token() {
        // The page ends exactly at the collection's end ⇒ no further page.
        let (items, token) = page(2, 2).apply(vec![10, 20, 30, 40]);
        assert_eq!(items, [30, 40]);
        assert_eq!(token, "");
    }

    #[test]
    fn apply_under_filled_last_page_has_no_next_token() {
        // A final page shorter than the page size ⇒ the listing is exhausted.
        let (items, token) = page(2, 10).apply(vec![10, 20, 30]);
        assert_eq!(items, [30]);
        assert_eq!(token, "");
    }

    #[test]
    fn apply_offset_past_total_yields_empty_page() {
        // An offset beyond the collection (a stale or over-skipped token) clamps to
        // the end: an empty page, no next token, no panic.
        let (items, token) = page(100, 10).apply(vec![10, 20, 30]);
        assert!(items.is_empty());
        assert_eq!(token, "");
    }

    #[test]
    fn split_overfetch_overfull_truncates_and_mints_token() {
        // The store handed back `size + 1` rows: the probe row proves a further page
        // exists. Truncate it off, mint the token advancing past this page.
        let (rows, token) = page(0, 2).split_overfetch(vec![10, 20, 30]);
        assert_eq!(rows, [10, 20]);
        assert_eq!(next_offset(&token), 2);
    }

    #[test]
    fn split_overfetch_exact_fit_has_no_next_token() {
        // Exactly `size` rows came back ⇒ no probe row ⇒ the listing is exhausted.
        let (rows, token) = page(0, 2).split_overfetch(vec![10, 20]);
        assert_eq!(rows, [10, 20]);
        assert_eq!(token, "");
    }

    #[test]
    fn split_overfetch_under_filled_page_has_no_next_token() {
        // A short final page (fewer than `size` rows) ⇒ exhausted.
        let (rows, token) = page(4, 2).split_overfetch(vec![50]);
        assert_eq!(rows, [50]);
        assert_eq!(token, "");
    }

    #[test]
    fn split_overfetch_empty_store_yields_empty_page() {
        // The store returned nothing ⇒ an empty page and no token.
        let (rows, token) = page(0, 2).split_overfetch(Vec::<u32>::new());
        assert!(rows.is_empty());
        assert_eq!(token, "");
    }
}
