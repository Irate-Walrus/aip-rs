//! AIP-158 pagination: page tokens and request checksums.
//!
//! Page tokens are encoded with `serde` + `postcard` + base64url (a 1-byte
//! version prefix guards against format drift) and are deliberately *not*
//! wire-compatible with `aip-go`'s gob tokens.
//! See `docs/adr/0004-page-token-encoding.md`.
//!
//! See <https://google.aip.dev/158>.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use prost_reflect::DynamicMessage;
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
    /// Parse and verify an offset page token from a request and its checksum.
    pub fn parse(_request: &impl PageRequest, _checksum: u32) -> Result<Self, Error> {
        todo!("decode token, verify checksum, apply skip")
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
/// fields (`page_token`, `page_size`, `skip`). Reflective.
pub fn request_checksum(_request: &DynamicMessage) -> Result<u32, Error> {
    todo!("clone, clear pagination fields, prost-encode, crc32 IEEE")
}

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
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
}
