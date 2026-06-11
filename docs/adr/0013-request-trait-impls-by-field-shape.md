# Emit pagination / ordering / filtering request-trait impls by field shape

The codegen plugin (ADR-0011) gains a second emission alongside the typed
resource-name wrappers: `impl aip_pagination::PageRequest`,
`impl aip_ordering::OrderByRequest`, and `impl aip_filtering::FilterRequest`
onto generated **request** messages. Emission keys on **field shape** — does
the message carry the AIP-standard fields? — not on the Standard-List **method
type** issue #115 originally proposed. It is off by default behind three
plugin flags.

## What changes

- **`aip-reflect` gains `RequestDescriptor` / `request_descriptors_in_file`**,
  mirroring the existing `ResourceDescriptor` pair. For each **top-level**
  message it records whether `string page_token`, `int32 page_size`,
  `int32 skip`, `string order_by`, and `string filter` are present. A field
  counts only when its name **and** type match and it is not proto3-`optional`
  (prost maps `optional int32 page_size` to `Option<i32>`, which a
  `fn page_size(&self) -> i32` body cannot return). Nested messages are
  ignored.
- **`GenInput` gains `requests: Vec<RequestDescriptor>`**; `aip-codegen` stays
  a pure, golden-tested function over descriptors. It emits `PageRequest` when
  `page_token` **and** `page_size` are present (adding a `skip()` override
  only when `skip` is too — otherwise the trait default of `0` stands),
  `OrderByRequest` when `order_by` is present, and `FilterRequest` when
  `filter` is — each independently. A name-match with the wrong type or
  proto3-`optional` is silently `false`: the message qualifies for fewer (or
  no) traits, and the missing impl surfaces at the generic call site
  (`fn parse_page<M: PageRequest>`), which points at the real fix.
- **Three plugin flags** — `pagination`, `ordering`, `filtering` — parsed from
  the previously-ignored `CodeGeneratorRequest.parameter` in the codebase's
  existing `key=value` `opt:` style. All default off; a disabled flag zeroes
  the matching bools in every `RequestDescriptor`, so `aip-codegen` never sees
  a flag. An unrecognized key or a value other than `true`/`false` is an
  **error** that fails `buf generate` — a typo must not silently disable
  emission.
- **Output lands in the same per-proto `<proto>.aip.rs`**, now emitted whenever
  the body carries a wrapper **or** an impl. A proto with no
  `google.api.resource` but a paginated request — freight's
  `freight_service.proto` — produces a file it did not before.
- **Generated `.aip.rs` become `use`-free and fully path-qualified**
  (`::aip_resourcename::…`, `::aip_pagination::PageRequest`, …). The consumer
  mounts every `.aip.rs` directly in the message module, so an
  `impl … for ListSitesRequest` names the prost struct by bare path. The
  per-file private submodule and flat re-export go away — one mount rule for
  all generated AIP code.

## Why

- **There is nothing in aip-go to mirror.** Go's `pagination.Request` is
  satisfied structurally — any message with `GetPageToken` / `GetPageSize`
  duck-types in, so `protoc-gen-go-aip` emits nothing. Rust traits are
  nominal: explicit impls are unavoidable, and freight hand-writes four today
  (`service.rs`). This feature moves them hand-written → generated.
- **Field shape is the Rust analog of Go's structural satisfaction.** Emitting
  the impl iff the fields the trait reads exist (a) guarantees the impl
  compiles and (b) covers Search (AIP-136), Batch (AIP-231..235), and custom
  paginated requests for free. Method-type detection fails both ways: it can
  emit over a List request missing a field, and it misses a paginated Search.
- **Flags are split and off by default.** Each impl adds its crate
  (`aip-pagination` / `aip-ordering` / `aip-filtering`) as a *direct*
  dependency of the consumer and would collide with hand-written impls. A tree
  may carry `order_by` yet sort another way; leaving `ordering` off withholds
  the impl. A crate only becomes a dependency where its impl is actually
  emitted.

## Considered Options

- **Method-type detection (issue #115's framing).** Rejected: nothing in
  aip-go to mirror, it can emit impls that fail to compile, and it misses
  Search/Batch/custom paginated requests. Field shape is more precise *and*
  broader.
- **A runtime-reflection blanket impl**
  (`impl<M: ReflectMessage> PageRequest for M`). Rejected: a borrowed
  `page_token(&self) -> &str` is awkward to serve reflectively, and one
  blanket impl cannot give `ListSitesRequest` its own `skip()` override.
- **Hard-error on a near-miss field** (right name, wrong type or `optional`).
  Rejected: it couples unrelated protos to this feature and fails the whole
  `buf generate`. (Flags *do* hard-error — the parameter string is owned by
  this plugin's user; other people's protos are not.)
- **`super::`-qualified per-file submodules, or a separate `.aip.traits.rs`.**
  Both avoid the mount change. Rejected: the `use`-free,
  mount-in-message-module form has the fewest moving parts and handles a mixed
  proto (resource + request in one file) with no special case.

## Consequences

- **Amends ADR-0011.** A second plugin output, a flag surface, the `use`-free
  generated format, and the new mount convention. Existing wrapper golden
  fixtures are re-blessed.
- **Footnote to ADR-0009.** The impls read fields by plain borrow
  (`&self.page_token`), not through a **Dynamic message**; `request_checksum`
  still reflects.
- **freight-server follow-up** (per the example-server rule): delete the four
  hand-written impls, set all three flags in `buf.gen.yaml`, add the three
  crates as direct dependencies, commit `freight_service.aip.rs`, and route
  filter handling through the new `FilterRequest` impls. `parse_page` is
  unchanged.
- **`MethodType` is unused by this feature** but stays in `aip-codegen` for
  the name/grammar codegen it was ported for (#62).
- **CONTEXT.md gains "Request descriptor"**, disambiguated from the reflection
  **Descriptor**.
- **Risk to verify in implementation:** that mounting every `.aip.rs` directly
  in the message module compiles for a **mixed** proto — wrapper struct and
  trait impl side by side, fully qualified, no name clash — and that
  fully-qualifying the wrapper bodies keeps the roundtrip golden tests green.
