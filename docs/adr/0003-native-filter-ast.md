# Native filter AST with optional CEL-proto conversion

`aip-go`'s `filtering` produces the `google.api.expr.v1alpha1` CEL `CheckedExpr`
protobuf message as its public output (`Filter.CheckedExpr`). The package parses
and type-checks but does not evaluate, so the AST *is* the product: users walk it
to build SQL/database queries. Prost-generated CEL types are awkward to construct
and pattern-match (every node a boxed `Option<oneof>`), and that tax would be paid
by every downstream consumer.

So aip-rs defines a native Rust enum AST tailored to the AIP-160 subset as the
primary representation, and offers CEL-proto interop (`From`/`Into` the
`v1alpha1` types) behind an optional `cel-proto` feature.

## Considered Options

- **Generate the CEL proto and expose it directly** (faithful) — best interop,
  worst ergonomics for the type users touch most.
- **Reuse an existing CEL crate's AST** — parses full CEL, not the AIP-160 subset,
  and we'd still need AIP-specific declarations/checker.

## Consequences

- We hand-maintain the native↔proto mapping and own an AST that must stay faithful
  to CEL semantics so conversions round-trip.
- The heavy generated CEL protos burden only users who enable `cel-proto`.
