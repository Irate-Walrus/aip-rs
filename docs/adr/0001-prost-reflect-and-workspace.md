# Use prost-reflect for reflection; organize as a workspace

aip-rs targets users already in the prost/tonic ecosystem, so we depend on
`prost-reflect` for the reflection-dependent features rather than abstracting
reflection behind our own trait or using rust-protobuf. To keep the pure-syntax
crates (`resourcename`, `resourceid`) free of that dependency weight, the project
is a Cargo workspace of per-feature crates re-exported by an umbrella `aip` crate;
`prost-reflect` is a hard dependency only of the crates that actually reflect
(`filtering`, `fieldmask`, `ordering`, `pagination`).

## Considered Options

- **Feature-gated `prost-reflect` in a single crate** — lean core via Cargo
  features. Rejected in favor of crate boundaries, which also give incremental
  compilation and let pure crates publish with zero proto deps.
- **Own reflection trait abstraction** — maximum decoupling, but the most design
  work and risks leaking protobuf semantics anyway.
- **rust-protobuf** — built-in reflection, but prost dominates the tonic
  ecosystem our users live in.

## Consequences

- A string-parsing SDK transitively pulls in a descriptor pool for its reflective
  crates — surprising at a glance, deliberate here.
- We can still feature-gate `prost-reflect` within a reflective crate later; that
  is a non-breaking change.
