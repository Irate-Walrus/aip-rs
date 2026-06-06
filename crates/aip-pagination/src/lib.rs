//! AIP-158 pagination: page tokens and request checksums.
//!
//! Page tokens are encoded with `serde` + `postcard` + base64url (a 1-byte
//! version prefix guards against format drift) and are deliberately *not*
//! wire-compatible with `aip-go`'s gob tokens.
//! See `docs/adr/0004-page-token-encoding.md`.
//!
//! See <https://google.aip.dev/158>.

use prost_reflect::DynamicMessage;
use serde::{Deserialize, Serialize};

/// Errors produced when parsing or verifying page tokens.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("malformed page token")]
    Malformed,
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
pub fn encode_page_token<T: Serialize>(_value: &T) -> String {
    todo!("postcard-serialize, version-prefix, base64url-encode")
}

/// Decodes an opaque page token into a cursor payload.
pub fn decode_page_token<T: serde::de::DeserializeOwned>(_token: &str) -> Result<T, Error> {
    todo!("base64url-decode, check version prefix, postcard-deserialize")
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
