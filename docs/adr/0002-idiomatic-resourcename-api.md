# Idiomatic resourcename API; code generation deferred

`aip-go`'s `resourcename` exposes variadic `Sscan`/`Sprint`, which exist as
targets for its `protoc-gen-go-aip` code generator (generated `XxxResourceName`
structs call them with positional `&field` lists). Rust has no variadics and we
ship no code generator in v0.1, so a literal port would import an ergonomic wart
that only ever served codegen.

Instead we replace them with a `regex::Captures`-style typed API: `Pattern::parse`
yields a reusable pattern, matching a resource name yields named captures
(`caps["shipper"]`). We keep the genuinely reusable free functions (`match`,
`ancestor`, `has_parent`, `contains_wildcard`, `validate`, `validate_pattern`)
and the segment-iteration types (`Scanner`, `Segment`, `Literal`).

## Consequences

- No positional `Sscan`/`Sprint`. Callers extract variables by name.
- A `#[derive(ResourceName)]` proc-macro (the Rust analog of `protoc-gen-go-aip`)
  may follow later in a separate crate without disturbing this runtime API.
