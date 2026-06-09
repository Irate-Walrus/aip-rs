# Adopt buf for the proto pipeline; generate resource names with a plugin

We switch the proto build from per-crate `protox` compilation to a single **buf**
pipeline: one `buf.yaml` module that takes `google.*` from
`buf.build/googleapis/googleapis` (digest-pinned in `buf.lock`), code emitted by
`buf generate` and **committed** to the tree. The typed resource-name wrappers
deferred by ADR-0002 are produced by a custom buf plugin, `protoc-gen-prost-aip`
— the Rust analog of aip-go's `protoc-gen-go-aip` — rather than the proc-macro
ADR-0002 floated. This supersedes ADR-0006's "No `protoc`" build and ADR-0002's
codegen-deferral, and resolves the vendored-proto duplication ADR-0006 itself
flagged ("a shared top-level `proto/` is the alternative if that drift becomes a
burden").

## What changes

- **buf replaces protox workspace-wide.** The four `build.rs` files
  (`test-fixtures`, `aip-iam`, `aip-filtering`, `freight-server`) and all vendored
  `google/**` are deleted. Generated Rust is committed, so downstream `cargo
  build` and docs.rs need **no toolchain** — only regeneration (`buf generate`)
  and the CI drift-check touch buf + the BSR.
- **New `aip-proto` crate** holds the standardized `google.*` types (`api` /
  `type` / `iam.v1`) plus CEL `expr.v1alpha1`, behind Cargo features (`iam`,
  `cel`, …) so an IAM-only user does not compile the CEL types. `google.*` comes
  from `buf.build/googleapis/googleapis`, pinned by digest in `buf.lock`.
- **Example/fixture protos stay local.** `einride.example.*` (the freight service,
  the syntax fixtures) is still generated in `freight-server` / `test-fixtures`,
  depending on `aip-proto` for the shared types — a library crate must not ship
  the demo's protos.
- **New `aip-codegen` crate** holds the codegen-only helpers `MethodType` /
  `GrammaticalName` / `strcase` (split out of #61 because they exist only to drive
  generation) **and** the generation logic (`generate(descriptors) ->
  Vec<GenFile>`). The plugin binary `protoc-gen-prost-aip` is a thin
  stdin→`generate()`→stdout shim, so the generator is golden-tested directly
  without spawning a process. The plugin reuses runtime `aip-reflect` to read the
  `google.api.resource` annotations — no reimplementation of the annotation
  parsing.

## Why

- The vendored googleapis tree was compiled **three times** and mirrored by hand
  on every edit — exactly the drift ADR-0006 predicted. One buf module with a
  managed dependency removes it.
- A plugin makes the **proto annotation the single source of truth** for resource
  names (aip-go parity), instead of hand-written structs (the proc-macro path)
  that must be kept in sync with the annotation.
- **`Policy` by construction.** With one `aip-proto`, `google.iam.v1.Policy`
  exists once, so `freight-server`'s eight `extern_path` mappings disappear and
  ADR-0010's "one `Policy`, not a structural duplicate" invariant is enforced by
  the compiler rather than a hand-maintained config list.
- buf preserves option/extension bytes in its image, so the
  `encode_file_descriptor_set` workaround in the protox `build.rs` files (needed
  because prost drops extension fields) is no longer required.

## Considered Options

- **Keep protox, add a `#[derive(ResourceName)]` proc-macro (ADR-0002's path).**
  Toolchain-free, but the resource-name struct duplicates the proto annotation and
  can drift, and it does nothing about the triplicated vendoring.
- **Keep protox, add a `build.rs` FDS generator.** Reads the protox
  `FileDescriptorSet` to emit resource names with no new toolchain — but still
  leaves the vendoring duplication and gives none of buf's lint / breaking-change
  / dependency management.
- **buf, but generate at build time (shell out from `build.rs`).** Reintroduces
  the toolchain requirement onto every consumer's `cargo build` — the
  openssl-sys-class distribution wound. Rejected in favour of committing the
  generated code.

## Consequences

- The build is **no longer hermetic inside `cargo` for maintainers**:
  regeneration needs the buf toolchain, and the CI drift-check (`buf generate &&
  git diff --exit-code`) needs network access to the BSR. Consumers are
  unaffected — the generated code is committed.
- **Amends ADR-0010.** `google.iam.v1.Policy` (and `Binding`, the audit/delta
  types, `google.type.Expr`) relocates from `aip-iam` to `aip-proto`. `aip-iam`
  keeps the structural read-modify-write ops — its core — and re-exports the types
  via `pub use aip_proto::…` so the public path `aip::iam::proto::…` is unchanged.
  The types no longer ride behind `aip-iam`'s own feature over vendored protox
  protos; they live in `aip-proto`.
- **Amends ADR-0001.** A new shared `aip-proto` crate sits under the per-feature
  crates. The pure crates (`aip-resourcename`, `aip-resourceid`) stay proto-free,
  and the `prost-reflect` choice and workspace shape are unchanged.
- `freight-server` **no longer "builds standalone"** (ADR-0006): it depends on
  `aip-proto` for the shared types and takes `google.*` from the BSR, though it
  still owns its `einride.example.freight.*` service protos.
- **Footnote to ADR-0009.** Descriptor pools stay per-crate and self-contained,
  built from buf's `--include-imports` image instead of the protox FDS; the
  Typed-facade / Dynamic-core decision is unaffected.
- **Risk to verify in implementation** (asserted from how buf / prost-reflect
  work, not yet proven here): cross-crate reflection — a freight message embedding
  an `aip-proto` `Policy`, with each crate's pool carrying its own copy of the
  imported `google.*` descriptors — and that the prost codegen plugin's emitted
  `file_descriptor_set` preserves extension bytes.
