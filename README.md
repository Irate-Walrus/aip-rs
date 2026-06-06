# aip-rs

A Rust SDK for implementing Google's [API Improvement Proposals (AIP)](https://google.aip.dev).

Rust port of [einride/aip-go](https://github.com/einride/aip-go).

## Features

- **Resource names** — parse and format AIP-122 resource name patterns.
- **Filtering** — parse and evaluate AIP-160 filter expressions.
- **Ordering** — parse AIP-132 `order_by` strings.
- **Pagination** — encode and decode AIP-158 page tokens.
- **Field masks** — apply and validate AIP-161 field masks.

## Install

```toml
[dependencies]
aip = "0.1"
```

## Example

[`examples/freight-server`](examples/freight-server) is a runnable tonic gRPC
server that demonstrates the crates end-to-end over einride's example
`FreightService`. It is a living demo that grows as each crate's issue lands:

```sh
cargo run -p freight-server
```

## Reference

The [`reference/`](reference) directory contains the upstream projects as git
submodules for easy consultation while porting:

- [`aip-go`](https://github.com/einride/aip-go) — the Go SDK this port is based on.
- [`google.aip.dev`](https://github.com/aip-dev/google.aip.dev) — the AIP specifications.

Clone with submodules via `git clone --recurse-submodules`, or run
`git submodule update --init` in an existing checkout.

## License

Licensed under the MIT License.
