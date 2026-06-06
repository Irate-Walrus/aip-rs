# Page tokens: serde + postcard, not wire-compatible with aip-go

`aip-go` encodes page tokens with Go's `gob` serializer, which is not portable to
other languages. Page tokens are opaque and persisted client-side, so
wire-compatibility with `aip-go` is neither achievable nor a goal. We therefore
choose the idiomatic Rust analog of `gob`: `serde` with the `postcard` binary
codec, base64url-encoded, with a 1-byte version prefix so a future format change
is detected rather than silently mis-decoded.

Arbitrary cursor tokens are supported generically via `T: Serialize +
DeserializeOwned`, matching the ergonomics of `aip-go`'s
`EncodePageTokenStruct`/`DecodePageTokenStruct` (the offset token is just one such
`T`). The request checksum — CRC32-IEEE over the prost-encoded request with
`page_token`/`page_size`/`skip` cleared — is ported faithfully and remains a
reflective `DynamicMessage` operation.

## Consequences

- aip-rs page tokens are deliberately incompatible with aip-go's; that is
  acceptable because tokens are opaque.
- Tokens are neither signed nor encrypted, matching aip-go: the checksum is a
  request-consistency guard, not a tamper-proof MAC. An optional signed/encrypted
  token codec could be added later without breaking the default.
