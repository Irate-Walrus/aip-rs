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

## License

Licensed under the MIT License.
