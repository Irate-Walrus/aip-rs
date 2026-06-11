# Adopt buf for the proto pipeline; generate resource names with a plugin

We switch the proto build from per-crate `protox` compilation to a single
**buf** pipeline: one `buf.yaml` module taking `google.*` from
`buf.build/googleapis/googleapis` (digest-pinned in `buf.lock`), code emitted by
`buf generate` and **committed** to the tree. The typed resource-name wrappers
deferred by ADR-0002 are produced by a custom buf plugin,
`protoc-gen-prost-aip` — the Rust analog of aip-go's `protoc-gen-go-aip` —
rather than the proc-macro ADR-0002 floated. This supersedes ADR-0006's "No
`protoc`" build and ADR-0002's codegen deferral, and resolves the
vendored-proto duplication ADR-0006 flagged.

## What changes

- **buf replaces protox workspace-wide.** The four `build.rs` files and all
  vendored `google/**` are deleted. Generated Rust is committed, so downstream
  `cargo build` and docs.rs need **no toolchain** — only regeneration
  (`buf generate`) and the CI drift-check touch buf + the BSR.
- **New `aip-proto` crate** holds the standardized `google.*` types (`api` /
  `type` / `iam.v1`) plus CEL `expr.v1alpha1`, behind Cargo features so an
  IAM-only user does not compile the CEL types.
- **Example/fixture protos stay local.** `einride.example.*` is still generated
  in `freight-server` / `test-fixtures`, depending on `aip-proto` for the
  shared types — a library crate must not ship the demo's protos.
- **New `aip-codegen` crate** holds the codegen-only helpers (`MethodType` /
  `GrammaticalName` / `strcase`) **and** the generation logic
  (`generate(descriptors) -> Vec<GenFile>`). The plugin binary
  `protoc-gen-prost-aip` is a thin stdin→`generate()`→stdout shim, so the
  generator is golden-tested without spawning a process; it reuses runtime
  `aip-reflect` to read the `google.api.resource` annotations.

## Why

- The vendored googleapis tree was compiled **three times** and mirrored by
  hand on every edit — exactly the drift ADR-0006 predicted. One buf module
  with a managed dependency removes it.
- A plugin makes the **proto annotation the single source of truth** for
  resource names (aip-go parity), instead of hand-written structs that must be
  kept in sync with it.
- **`Policy` by construction.** With one `aip-proto`, `google.iam.v1.Policy`
  exists once: `freight-server`'s eight `extern_path` mappings disappear and
  ADR-0010's "one `Policy`" invariant is enforced by the compiler.
- buf preserves option/extension bytes in its image, so the
  `encode_file_descriptor_set` workaround in the protox `build.rs` files
  (prost drops extension fields) is no longer required.

## Considered Options

- **Keep protox, add a `#[derive(ResourceName)]` proc-macro** (ADR-0002's
  path) — toolchain-free, but the struct duplicates the proto annotation and
  can drift, and it does nothing about the triplicated vendoring.
- **Keep protox, add a `build.rs` FDS generator** — no new toolchain, but
  leaves the vendoring duplication and none of buf's lint / breaking-change /
  dependency management.
- **buf, generating at build time** (shell out from `build.rs`) — reintroduces
  the toolchain requirement onto every consumer's `cargo build`, the
  openssl-sys-class distribution wound. Rejected in favour of committing the
  generated code.

## Consequences

- The build is **no longer hermetic inside `cargo` for maintainers**:
  regeneration needs the buf toolchain, and the CI drift-check
  (`buf generate && git diff --exit-code`) needs BSR network access. Consumers
  are unaffected — the generated code is committed.
- **Amends ADR-0010.** `google.iam.v1.Policy` (and `Binding`, the audit/delta
  types, `google.type.Expr`) relocates from `aip-iam` to `aip-proto`; `aip-iam`
  keeps its structural ops and re-exports the types so the public path
  `aip::iam::proto::…` is unchanged.
- **Amends ADR-0001.** A shared `aip-proto` crate sits under the per-feature
  crates; the pure crates (`aip-resourcename`, `aip-resourceid`) stay
  proto-free, and the `prost-reflect` choice and workspace shape are unchanged.
- `freight-server` **no longer "builds standalone"** (ADR-0006): it depends on
  `aip-proto` and takes `google.*` from the BSR, though it still owns its
  `einride.example.freight.*` service protos.
- **Footnote to ADR-0009.** Descriptor pools stay per-crate and self-contained,
  built from buf's `--include-imports` image instead of the protox FDS; the
  Typed-facade / Dynamic-core decision is unaffected.
- **Risk verified in implementation:** cross-crate reflection (a freight
  message embedding an `aip-proto` `Policy`, each crate's pool carrying its own
  copy of the imported `google.*` descriptors) and extension-byte preservation
  in the plugin's emitted `file_descriptor_set`.
