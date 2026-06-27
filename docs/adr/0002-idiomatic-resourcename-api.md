# Idiomatic resourcename API; code generation deferred

> **Partially superseded by [ADR-0011](0011-buf-proto-pipeline.md).** The
> deferred-codegen decision below — and the `#[derive(ResourceName)]` proc-macro
> it floats — is resolved by ADR-0011, which generates resource names with the
> `protoc-gen-prost-aip` buf plugin instead. The runtime `resourcename` API
> described here stands unchanged; the generated wrappers layer on it.

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

## Amendment (ADR-0016): match a parent with wildcards, binding each variable

The typed-key store (ADR-0016) scopes a list by composing one equality predicate
per concrete parent **Segment** and omitting it per **Wildcard**. A handler needs
the per-**Variable** bindings to do that: which **Variables** the parent fixed,
and to what. The existing `regex::Captures`-style match yields named captures for
a *concrete* **Resource name**, and the existing `contains_wildcard` answers a
*boolean* about a name; neither hands back the bindings of a name that mixes
concrete **Segments** with `-` **Wildcards**.

**Decision:** add
`Pattern::match_with_wildcards(name) -> Result<HashMap<&str, Option<&str>>, Error>`.
It matches a **Resource name** against the **Pattern**, tolerating a `-`
**Wildcard** in any **Variable** position, and binds each **Variable** to
`Some(resource_id)` for a concrete **Segment** or `None` for a **Wildcard**. A
handler reads it as `<Parent>::pattern().match_with_wildcards(parent)?` and turns
each `Some` into a scope predicate, dropping each `None` (ADR-0008 amendment). It
sits beside the existing `match` / `contains_wildcard` / `is_wildcard` surface and
keeps the typed, by-name extraction style — the difference is the `Option` per
**Variable**, which is exactly the concrete-or-wildcard distinction the scope
needs.

**Consequences.** The runtime API stays name-by-name and reflection-free; no
positional surface returns. The store layer never reasons about `-` itself — it
consumes the `None`s this method produces, so the single-segment **Wildcard**
semantics are decided here, in the resource-name layer, not at a SQL-string
boundary.
