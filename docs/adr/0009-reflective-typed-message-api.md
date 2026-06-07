# Reflective primitives over ReflectMessage-typed messages, layered on a dynamic core

The **Reflective primitives** — applying an **Update mask** (`aip-fieldmask`) and
computing a **Page token**'s request checksum (`aip-pagination`) — were expressed
over `prost_reflect::DynamicMessage`/`MessageDescriptor`. But every realistic
caller holds concrete generated prost types, so each one had to hand-roll the same
**Dynamic message** bridge: look the **Descriptor** up by name in a pool it built
itself, wire-encode the concrete value into a `DynamicMessage`, run the primitive,
wire-decode back. The example carried that bridge as `reflect.rs`; deleting it
would not have removed the work, only moved it to the next server — the signature
of a deep module the library was missing.

We lift the bridge into the library and put the seam in the type system: the
headline interface of each reflective primitive — its **Typed facade** — is
generic over `prost_reflect::ReflectMessage`, so the **Descriptor** travels with
the value. There is no descriptor pool to build or thread, and `DynamicMessage`
does not appear at the headline interface. The facade is layered on a still-public
**Dynamic core** (the existing `DynamicMessage`/`MessageDescriptor` interface),
which remains the low-level escape hatch and the crates' test surface (JSON → a
**Dynamic message** via `test-fixtures`, so ADR-0006 is unchanged).

## The shape

- **`pagination::request_checksum<M: ReflectMessage>(req: &M)`** — a single
  generic function, *not* a facade/core pair. It only reads (transcode → clear
  `page_token`/`page_size`/`skip` → CRC), so it needs no `Default` and no
  decode-back. Because `DynamicMessage` itself implements `ReflectMessage`, the
  old `&DynamicMessage` callers and tests keep compiling unchanged.
- **`fieldmask::update<M: ReflectMessage + Default>(mask, dst: &mut M, src: &M)`**
  — a true **Typed facade** over the renamed **Dynamic core**
  `update_dynamic(mask, dst: &mut DynamicMessage, src: &DynamicMessage)`. The
  facade transcodes `src`/`dst`, runs the core, and decodes `dst` back. The pair
  is genuine: the decode-back needs `M: Default`, which `DynamicMessage` lacks, so
  the two paths cannot unify. The headline name `update` goes to the facade (the
  leverage); the core takes the qualified `update_dynamic`.
- **`fieldmask::validate(mask, descriptor: &MessageDescriptor)`** — kept as is. A
  `MessageDescriptor` is the reified type, not a value in dynamic clothing, and
  validation is a type-level operation often done before any instance exists. No
  `validate_for::<M>()` sugar for now: it would only save
  `&M::default().descriptor()` at the call site (deletion test — a trivial
  one-liner reappears), so we add it only if real call sites prove noisy.
- **Transcode is an invariant, not a `Result`.** The `T → DynamicMessage → T`
  round-trip can fail only if a type and its descriptor disagree — a build/config
  bug, not bad input — so the facade `expect()`s it rather than minting an error
  variant that cannot be exercised.

## The build requirement

A **Typed message** carries its **Descriptor** only if it is generated to do so.
So consumers must generate their protos with `prost-reflect-build` (descriptor
embedding + `ReflectMessage` derives), driven off the `protox`-built
`FileDescriptorSet` — **not** `prost-reflect-build`'s default `compile_protos`,
which would invoke `protoc`. The pure-Rust, no-`protoc` property of ADR-0001/0006
is preserved because `protox` remains the compiler and only adds the
`ReflectMessage` attributes + descriptor-pool static to the existing `compile_fds`
codegen.

Fallback if `prost-reflect-build` cannot cleanly consume the protox FDS: the
derive is just an attribute, so we add it through the `Config` already handed to
`compile_fds_with_config` (the per-message `message_name` is all
`prost-reflect-build` was automating). Either way no `protoc` is reintroduced.

## Considered Options

- **Caller passes a `DescriptorPool` per call** — keeps the lighter `prost::Name`
  bound the example already meets and needs no codegen change, but the pool rides
  on every signature (least deep) and `DynamicMessage` work stays caller-side.
- **A `Reflector` handle owning the pool** — clean per-call interface with the
  pool supplied once, no build change, but introduces a shared adapter and still
  carries the pool as a runtime object. Rejected for the `ReflectMessage` bound,
  which removes the pool from the interface entirely at the cost of the build
  requirement above.
- **Replace the dynamic interface outright** (typed only, `DynamicMessage`
  private) — one clean face, but the reflective crates could no longer test
  through JSON → `DynamicMessage` (needs generated types in tests, against
  ADR-0006) and JSON/gateway callers lose any entry point. Rejected in favor of
  layering.

## Consequences

- This refines ADR-0001, it does not overturn it: `prost-reflect` stays a hard
  dependency of the reflective crates, but the audience narrows from "users in the
  prost/tonic ecosystem" to "users who generate descriptors." A vanilla
  `tonic-prost-build` consumer must change codegen before calling these
  primitives — recorded here so it is a decision, not a surprise.
- The example loses its hand-rolled reflection: `reflect.rs` is deleted,
  `proto.rs`'s hand-built `DESCRIPTOR_POOL` becomes the generated one, and
  `service.rs`'s `to_dynamic`/`from_dynamic`/`request_checksum_of` dances collapse
  into direct typed calls.
- The **Dynamic core** stays public, so `test-fixtures` (ADR-0006) and any
  `DynamicMessage`-holding caller keep a supported path, and the crates' existing
  reflective tests are unaffected.
- A `build.rs` spike should confirm the `prost-reflect-build` + `protox` FDS wiring
  before the conversion lands; the fallback above bounds the risk.
