# Reflective primitives over ReflectMessage-typed messages, layered on a dynamic core

The **Reflective primitives** — applying an **Update mask** (`aip-fieldmask`)
and computing a **Page token**'s request checksum (`aip-pagination`) — were
expressed over `prost_reflect::DynamicMessage`/`MessageDescriptor`. But every
realistic caller holds concrete generated prost types, so each one had to
hand-roll the same bridge: build a descriptor pool, wire-encode into a
`DynamicMessage`, run the primitive, decode back. The example carried that
bridge as `reflect.rs`; deleting it would only have moved the work to the next
server — the signature of a deep module the library was missing.

We lift the bridge into the library and put the seam in the type system: each
reflective primitive's headline interface — its **Typed facade** — is generic
over `prost_reflect::ReflectMessage`, so the **Descriptor** travels with the
value. No descriptor pool is built or threaded, and `DynamicMessage` does not
appear at the headline interface. The facade layers on a still-public **Dynamic
core** (the existing `DynamicMessage`/`MessageDescriptor` interface), which
remains the low-level escape hatch and the crates' test surface (JSON → a
**Dynamic message** via `test-fixtures`, so ADR-0006 is unchanged).

## The shape

- **`pagination::request_checksum<M: ReflectMessage>(req: &M)`** — a single
  generic function, not a facade/core pair: it only reads, so it needs no
  `Default` and no decode-back. `DynamicMessage` itself implements
  `ReflectMessage`, so the old callers and tests compile unchanged.
- **`fieldmask::update<M: ReflectMessage + Default>(mask, dst, src)`** — a true
  **Typed facade** over the **Dynamic core** `update_dynamic`. The facade
  transcodes, runs the core, and decodes `dst` back; the decode-back needs
  `M: Default`, which `DynamicMessage` lacks, so the two paths cannot unify.
  The headline name goes to the facade; the core takes the qualified name.
- **`fieldmask::validate(mask, descriptor)`** — kept as is. A
  `MessageDescriptor` is the reified type, and validation is a type-level
  operation often done before any instance exists. No `validate_for::<M>()`
  sugar: it would only save `&M::default().descriptor()` at the call site.
- **Transcode is an invariant, not a `Result`.** The `T → DynamicMessage → T`
  round-trip fails only if a type and its descriptor disagree — a build bug,
  not bad input — so the facade `expect()`s it rather than minting an
  unexercisable error variant.

## The build requirement

A **Typed message** carries its **Descriptor** only if generated to do so, so
consumers must generate with descriptor embedding + a `ReflectMessage` impl per
message (originally `prost-reflect-build` over the `protox`-built
`FileDescriptorSet`; since ADR-0011, the buf pipeline). No `protoc` is
reintroduced.

### Emitting the `ReflectMessage` impls (issue #191)

The impls were first wired by `prost_reflect`'s **derive**, which forces
`#[prost_reflect(message_name = "<fqn>")]` as a compile-time literal on every
message. `protoc-gen-prost`'s `type_attribute` opt cannot interpolate the FQN, so
each consumer's `buf.gen.yaml` hand-listed one attribute line per message — 66 in
`aip-proto`, growing with every schema bump, and past the BSR remote-execution
param limit at scale. We instead **emit the impl directly** from
`protoc-gen-prost-aip` (the same `.aip.rs` mechanism as the resource-name
wrappers, `PageRequest`, and `SoftDeletable`): with `reflect=true` it writes an
`impl ::prost_reflect::ReflectMessage` for every generated message, resolving the
descriptor from a configurable `reflect_descriptor_pool` at runtime. The plugin
already receives every message's FQN, so the list is no longer hand-maintained.

Key points of the emission:

- **Every message, nested included** — the walk recurses and skips only the
  synthetic map-entry messages (no generated struct). A nested proto message
  `pkg.A.B.C` is named by its prost Rust path `a::b::C`, snake-casing the parent
  modules with prost-build's exact keyword rule (`Type` -> `r#type`).
- **`reflect_descriptor_pool` is required when `reflect=true`** — the two in-repo
  consumers disagree on the path (`crate::DESCRIPTOR_POOL` vs
  `crate::proto::DESCRIPTOR_POOL`), so there is no default; a missing one fails
  generation rather than guessing.
- **Completeness stays compiler-enforced** — a wrong Rust path or a missing impl
  is a build error in the consumer (the property the derive's required
  `message_name` gave), not silent drift.
- **`prost_reflect` is named directly** (`::prost_reflect`), not routed through
  the `aip` umbrella: it is a third-party crate the consumer already depends on.
- **Scope is the consumer's own packages.** The plugin generates for whatever is
  in `file_to_generate`, so a consumer that extern-paths shared packages (e.g.
  freight's `google.*` onto `aip-proto`) must keep those out of the aip plugin's
  inputs — done with per-plugin `include_imports: false` so it never emits an
  impl for a type whose struct lives in another crate.

## Considered Options

- **Caller passes a `DescriptorPool` per call** — no codegen change, but the
  pool rides on every signature (least deep) and the `DynamicMessage` work
  stays caller-side.
- **A `Reflector` handle owning the pool** — clean per-call interface, but a
  shared adapter carrying the pool as a runtime object. Rejected for the
  `ReflectMessage` bound, which removes the pool from the interface entirely at
  the cost of the build requirement above.
- **Replace the dynamic interface outright** (typed only) — one clean face, but
  the reflective crates could no longer test through JSON → `DynamicMessage`
  (needs generated types in tests, against ADR-0006) and JSON/gateway callers
  lose any entry point. Rejected in favor of layering.

For the impl-emission (issue #191):

- **Keep the prost derive + hand-listed `message_name`** — no plugin change, but
  the list is hand-maintained, grows with the schema, and overflows the BSR
  remote-execution param limit. Rejected for the plugin emission above.
- **`prost-reflect-build` in a `build.rs`** — generates the impls from the
  `FileDescriptorSet` at build time, but reintroduces a codegen `build.rs` and a
  proto toolchain at consumer build, which ADR-0011 deliberately removed (the
  output is committed). Rejected.

## Consequences

- Refines ADR-0001 without overturning it: `prost-reflect` stays a hard
  dependency of the reflective crates, but the audience narrows from "users in
  the prost/tonic ecosystem" to "users who generate descriptors." A vanilla
  `tonic-prost-build` consumer must change codegen before calling these
  primitives — recorded here so it is a decision, not a surprise.
- The example loses its hand-rolled reflection: `reflect.rs` is deleted, the
  hand-built `DESCRIPTOR_POOL` becomes the generated one, and the
  `to_dynamic`/`from_dynamic` dances collapse into direct typed calls.
- The **Dynamic core** stays public, so `test-fixtures` (ADR-0006) and any
  `DynamicMessage`-holding caller keep a supported path.
