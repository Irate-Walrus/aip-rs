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

## Amendment (ADR-0016): a cursor page token at version 2

The typed-key store (ADR-0016) pages with a **Cursor page token** — the last
row's ordered values plus a `(key columns) > (…)` seek — in place of the **Offset
page token**. The version byte built in above is exactly the mechanism that makes
the swap safe: it is the difference between a detected, cleanly-rejected format
change and a silent mis-decode.

**Decision:** bump the version byte from `1` to `2` and swap the inner payload.

- The payload's `offset: i64` is replaced by `cursor: Vec<CursorEntry>`, a
  self-describing list of `(column, value)` in `ORDER BY` clause order;
  `request_checksum: u32` keeps its role unchanged. `CursorEntry { column, value }`
  carries a `CursorValue`, a closed sum
  (`Bool | Int(i64) | Double(f64) | Text(String) | Bytes(Vec<u8>)`) defined in
  `aip-pagination` so the leaf crate stays free of `aip-sql`. Null is not a cursor
  value; **Timestamps** ride RFC3339 in `Text` and proto **enums** ride as their
  value name in `Text`.
- A version-`1` offset token presented to a version-`2` server fails the version
  check and is rejected; the client restarts pagination (AIP-158). Old and new
  tokens never silently cross.
- Decode keeps the `request_checksum` guard (CRC32-IEEE over the prost-encoded
  request with the pagination fields cleared) and adds two cross-checks: the
  cursor's column list must equal the resolved `ORDER BY` column list position by
  position, and each value's variant must match the **Schema**-allowlisted **Type**
  of its column (`Schema::column_type`, ADR-0008 amendment). Any failure is
  `INVALID_ARGUMENT`.

**Consequences.** Tokens stay opaque, unsigned, and unencrypted — the stance above
is unchanged; the self-describing column names are a debuggability and
decode-cross-check choice, accepted as redundant with the checksum (ADR-0016
records the rejected positional alternative). The generic `T: Serialize +
DeserializeOwned` codec is untouched; the cursor payload is simply one more such
`T`, and the offset payload is retired with freight, its only consumer.
