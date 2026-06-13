# aip-rs

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
![Status: pre-release](https://img.shields.io/badge/status-pre--release-orange.svg)

A Rust SDK for implementing [Google's API Improvement Proposals](https://google.aip.dev)
(AIP) — resource names, filtering, ordering, pagination, field masks, etags,
and friends — ported from [`einride/aip-go`](https://github.com/einride/aip-go).

> [!WARNING]
> **Development of this project is AI-assisted.** Large parts of the code,
> tests, and documentation are written with the help of AI tooling, with human
> review. Evaluate the code to your own standards before depending on it.

> [!NOTE]
> Not published to crates.io. Depend on it via git for now (see
> [Installation](#installation)) or better yet, not at all.

## Overview

Each AIP concern lives in its own focused crate, re-exported by the umbrella
`aip` crate behind a feature flag of the same name:

| Crate                | AIP                                                          | What it does                                                          |
| -------------------- | ------------------------------------------------------------ | ---------------------------------------------------------------------- |
| `aip-resourcename`   | [AIP-122](https://google.aip.dev/122)                         | Parse, match, and format resource name patterns                        |
| `aip-resourceid`     | [AIP-122](https://google.aip.dev/122)                         | Validate user-settable and generate system resource IDs                |
| `aip-requestid`      | [AIP-155](https://google.aip.dev/155)                         | `request_id` validation and the idempotency contract                   |
| `aip-pagination`     | [AIP-158](https://google.aip.dev/158)                         | Page token encode/decode with a request-consistency checksum           |
| `aip-filtering`      | [AIP-160](https://google.aip.dev/160)                         | Parse and evaluate filter expressions                                  |
| `aip-fieldmask`      | [AIP-161](https://google.aip.dev/161)                         | Apply and validate field masks                                         |
| `aip-ordering`       | [AIP-132](https://google.aip.dev/132)                         | Parse `order_by` strings                                               |
| `aip-fieldbehavior`  | [AIP-203](https://google.aip.dev/203)                         | Enforce `field_behavior` annotations (`REQUIRED`, `OUTPUT_ONLY`, …)     |
| `aip-etag`           | [AIP-154](https://google.aip.dev/154)                         | Content etags and the stale/malformed freshness check                   |
| `aip-softdelete`     | [AIP-164](https://google.aip.dev/164)/[165](https://google.aip.dev/165) | Soft delete / undelete state rules and the purge contract     |
| `aip-preview`        | [AIP-163](https://google.aip.dev/163)                         | `validate_only` preview gate                                            |
| `aip-validation`     | [AIP-193](https://google.aip.dev/193)                         | Accumulate per-field violations into one rich error                     |
| `aip-errordomain`    | [AIP-193](https://google.aip.dev/193)                         | tonic/tower layer stamping the service's `ErrorInfo.domain`             |
| `aip-reflect`        | [AIP-123](https://google.aip.dev/123)                         | Resource-annotation reflection over prost descriptors                   |
| `aip-iam`            | —                                                             | IAM primitives: member/role/permission parsing, `Policy` (opt-in)       |
| `aip-sql`            | —                                                             | Transpile the filter AST to a parameterized SQL predicate (opt-in)      |
| `aip-codegen`        | [AIP-122](https://google.aip.dev/122)                         | Generate typed resource-name wrappers (`protoc-gen-prost-aip` plugin)   |

With the optional `tonic` feature, every crate's `Error` converts into a
`tonic::Status` carrying the correct gRPC code and
[AIP-193](https://google.aip.dev/193) standard error details (`ErrorInfo` +
`BadRequest`) — a bare `?` in a handler produces the right status.

## Installation

```toml
[dependencies]
aip = { git = "https://github.com/irate-walrus/aip-rs" }
```

The default features enable the core crates. Trim to exactly what you need:

```toml
# Minimal: only resource-name parsing.
aip = { git = "https://github.com/irate-walrus/aip-rs", default-features = false, features = ["resourcename"] }

# Everything plus rich gRPC statuses for tonic servers.
aip = { git = "https://github.com/irate-walrus/aip-rs", features = ["tonic"] }
```

The minimum supported Rust version (MSRV) is **1.95**, declared in
`Cargo.toml` and enforced in CI.

## Quick start

Parse, match, and format an [AIP-122](https://google.aip.dev/122) resource
name pattern:

```rust
use aip::resourcename::Pattern;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pattern = Pattern::parse("shippers/{shipper}/sites/{site}")?;

    // Match a concrete name and pull out its variables.
    let captures = pattern
        .match_name("shippers/acme/sites/sydney")
        .expect("name matches pattern");
    assert_eq!(captures.get("shipper"), Some("acme"));
    assert_eq!(captures.get("site"), Some("sydney"));

    // Format a name from (variable, value) pairs.
    let name = pattern.format([("shipper", "acme"), ("site", "sydney")])?;
    assert_eq!(name, "shippers/acme/sites/sydney");

    Ok(())
}
```

See the crate-level docs (`cargo doc --open -p aip`) for a worked
update-handler example combining pagination, field masks, etags, and the
`tonic` error mapping.

## Example server

[`examples/freight-server`](examples/freight-server) is a runnable tonic gRPC
server demonstrating the crates end-to-end over einride's example
`FreightService`. It grows with the library — every feature lands there too:

```sh
cargo run -p freight-server
```

Its [README](examples/freight-server/README.md) has copy-paste `grpcurl`
commands and a status table of implemented RPCs.

## Code generation plugin

`protoc-gen-prost-aip` is the buf/`protoc` plugin that generates typed
resource-name wrappers (and the field-shape-keyed request-trait impls) from your
`google.api.resource` annotations — the Rust analog of
[`protoc-gen-go-aip`](https://github.com/einride/aip-go) (ADR-0011). You only
need it to *regenerate* code; consumers of the generated crates need nothing.

Install the prebuilt binary for your platform from a
[release](https://github.com/irate-walrus/aip-rs/releases) (tags
`protoc-gen-prost-aip-v*`, with binaries for linux and macOS on x86_64 and
aarch64), and put it on `PATH` so `buf` can find it:

```sh
# Pick the asset for your platform, then:
tar xzf protoc-gen-prost-aip-*-x86_64-unknown-linux-gnu.tar.gz
install -m755 protoc-gen-prost-aip ~/.local/bin/   # anywhere on $PATH

# buf discovers `protoc-gen-prost-aip` on PATH via a `local:` plugin entry.
buf generate
```

Or build it from source with `cargo install --git
https://github.com/irate-walrus/aip-rs protoc-gen-prost-aip`. See
[`examples/freight-server/buf.gen.yaml`](examples/freight-server/buf.gen.yaml)
for a worked plugin invocation.

## Documentation

- API docs: `cargo doc --open -p aip` (not yet on docs.rs)
- [`CONTEXT.md`](CONTEXT.md) — the domain glossary used across the codebase
- [`docs/adr/`](docs/adr) — architecture decision records for each subsystem

## Reference

The [`reference/`](reference) directory contains the upstream projects as git
submodules for easy consultation while porting:

- [`aip-go`](https://github.com/einride/aip-go) — the Go SDK this port is based on
- [`google.aip.dev`](https://github.com/aip-dev/google.aip.dev) — the AIP specifications

Clone with submodules via `git clone --recurse-submodules`, or run
`git submodule update --init` in an existing checkout.

## Contributing

Issues and pull requests are welcome. Before changing a subsystem, read the
relevant ADR in [`docs/adr/`](docs/adr) and use the terminology from
[`CONTEXT.md`](CONTEXT.md). A change is complete when
[`examples/freight-server`](examples/freight-server) exercises it.

### Releasing

Releasing is **paused** while the API stabilises — everything stays at `0.0.1`,
with no automatic version bumps or releases. The MSRV job and the rest of CI run
as normal.

The machinery is in place and dormant: the [release-plz](https://release-plz.dev)
workflow ([`.github/workflows/release-plz.yml`](.github/workflows/release-plz.yml))
is `workflow_dispatch`-only, and [`release-plz.toml`](release-plz.toml) is set to
git-only (`publish = false`). To re-enable, restore the `push` trigger; to publish
to crates.io later, the `aip` and `aip-filtering` names (owned by unrelated crates
there) must be renamed and a token added, as documented in those two files.

## Credits

This project is a Rust port of
[`go.einride.tech/aip`](https://github.com/einride/aip-go) by
[Einride](https://github.com/einride); the API shapes and test corpus follow
the Go SDK closely.

## License

Licensed under the [MIT License](LICENSE).
