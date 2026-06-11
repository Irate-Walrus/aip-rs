# Emit pagination / ordering request-trait impls by field shape

The codegen plugin (ADR-0011) gains a second emission alongside the typed
resource-name wrappers: `impl aip_pagination::PageRequest` and
`impl aip_ordering::OrderByRequest` onto generated **request** messages. It keys
emission on **field shape** — a message that carries the AIP-standard pagination
/ ordering fields — **not** on **method type**, the Standard-List-method identity
this issue (#115) originally proposed. The emission is off by default behind two
plugin flags. This **reverses the issue's framing**, for the reason below.

## What changes

- **`aip-reflect` gains a `RequestDescriptor` + `request_descriptors_in_file`.**
  The runtime field-shape digest, mirroring the existing `ResourceDescriptor` /
  `resource_descriptors_in_file` pair. Per **top-level** message it records the
  presence of `string page_token`, `int32 page_size`, `int32 skip`, and
  `string order_by` — each `true` only when the field's name **and** type match
  and it is not proto3-`optional` (a `optional int32 page_size` becomes
  `Option<i32>` in prost, which a `fn page_size(&self) -> i32` body cannot
  return, so it must not count). Nested messages are not considered.
- **`GenInput` gains `requests: Vec<RequestDescriptor>`.** `aip-codegen` stays a
  pure function over descriptors (golden-tested directly): it emits `PageRequest`
  when `page_token && page_size`, adds a `skip()` override only when `skip` is
  present (else the trait default of `0` stands), and emits `OrderByRequest`
  when `order_by` is present — each independently.
- **Two plugin flags, parsed from the `CodeGeneratorRequest.parameter`** field
  (previously ignored): `pagination=true` and `ordering=true`, both **off by
  default** (absent ⇒ off), in the codebase's existing `key=value` `opt:` style.
  The plugin gates the bools it writes into each `RequestDescriptor`:
  `ordering=false` zeroes `order_by`; `pagination=false` zeroes
  `page_token` / `page_size` / `skip`. `aip-codegen` never sees a flag — it emits
  purely from the bools.
- **A name-match with the wrong type or proto3-`optional` is silently skipped**
  (its bool is `false`), so the message may fail to qualify (or qualify for fewer
  traits). Generation never errors on a near-miss; a missing impl surfaces later
  at the generic call site (`fn parse_page<M: PageRequest>`), which points at the
  real fix.
- **Output placement: the same per-proto `<proto>.aip.rs`.** `generate_file`
  emits a file whenever its body carries **either** a wrapper **or** an impl
  (today it returns `Ok(None)` unless a patterned resource is present). A proto
  with no `google.api.resource` but a paginated request — e.g. freight's
  `freight_service.proto` — now produces a `freight_service.aip.rs` it did not
  before.
- **All generated `.aip.rs` become `use`-free and fully path-qualified.** The
  wrapper header `use aip_resourcename::{Error, Pattern};` is dropped in favour of
  inline `::aip_resourcename::…`, and the impls name their traits
  `::aip_pagination::PageRequest` / `::aip_ordering::OrderByRequest`. The consumer
  then mounts **every** `.aip.rs` directly in the message module, so an
  `impl … for ListSitesRequest` names the prost struct by bare path. The per-file
  private submodule and flat re-export (`mod site_aip { … } pub use
  site_aip::SiteResourceName;`) go away — one mount rule for all generated AIP
  code.

## Why

- **aip-go emits no such impls — there is nothing to mirror.** Go's
  `pagination.Request` is satisfied *structurally* (any message with
  `GetPageToken` / `GetPageSize` duck-types into it); `skip` is a runtime type
  assertion. So ADR-0011's "mirror `protoc-gen-go-aip`" rationale, which justifies
  the resource-name wrappers, does **not** reach pagination/ordering: the Go
  generator never produces them. The ported `MethodType` is real, but it exists to
  drive name/grammar codegen (#62), not this feature.
- **Rust traits are nominal, so impls are unavoidable.** A handler generic over
  `M: PageRequest` (freight's `parse_page`) cannot be called without an explicit
  impl per request type. Today freight hand-writes them (`service.rs` —
  `ListShippersRequest`, `ListSitesRequest` + `skip`, `ListShipmentsRequest`,
  `OrderByRequest for ListSitesRequest`). This feature moves them
  hand-written → generated; it does not make them optional.
- **Field shape is the Rust analog of Go's structural satisfaction.** Emit the
  impl *iff* the fields the trait reads exist. That (a) guarantees the emitted impl
  compiles — you never emit a `page_size()` body over a message lacking
  `page_size` — and (b) covers Search (AIP-136) / Batch (AIP-231..235) / custom
  paginated requests for free, since they carry the same fields. Method-type
  detection does the opposite on both counts: it can emit over a List request that
  is missing a field (then fails to compile) and misses a paginated Search request
  (wrong method identity).
- **Split flags, off by default.** The impls add `aip-pagination` /
  `aip-ordering` as *direct* dependencies of the consumer and would collide with
  any hand-written impls, so emission is opt-in. The flags are split because a tree
  may carry `order_by` fields yet not want `OrderByRequest` (it sorts another way);
  `ordering=false` withholds it. A pagination-only tree never pulls `aip-ordering`
  even so, because the ordering `use`/impl is only emitted where `order_by` is
  actually present.

## Considered Options

- **Method-type detection (this issue's original proposal).** Carry Standard-List
  method identity (via `MethodType`) into `GenInput` and emit onto each List
  method's request. Rejected: nothing in aip-go to mirror (Go duck-types), it can
  emit an impl that fails to compile when a conventional field is absent, and it
  misses Search/Batch/custom paginated requests. Field shape is strictly more
  precise *and* broader.
- **A runtime reflection blanket impl** — `impl<M: ReflectMessage> PageRequest
  for M`, reading the fields off a `DynamicMessage`. One impl, no codegen.
  Rejected: a borrowed `page_token(&self) -> &str` is awkward to serve from a
  reflective read, and a single blanket impl is a coherence lock-in — it cannot
  give `ListSitesRequest` its own `skip()` override. freight already chose typed
  impls "without reflection"; ADR-0009 made the types Typed for the request
  checksum, but field access stays a plain borrow.
- **Hard-error on a name-match with the wrong type / proto3-`optional`.**
  Rejected: it couples unrelated protos (a stray `string skip` on some message) to
  this feature and fails the whole `buf generate`. Silent skip leaves the gap to
  surface at the `parse_page` call site.
- **`super::`-qualify the message from a per-file submodule, or emit a separate
  `.aip.traits.rs`.** Both keep the impls compiling without the mount change.
  Rejected in favour of the `use`-free / mount-in-message-module form: it has the
  fewest moving parts, couples no generated code to the consumer's mount depth, and
  handles a mixed file (resource + request in one proto) with no special case.

## Consequences

- **Amends ADR-0011.** The plugin gains a second output (request-trait impls) and
  a flag surface (the `parameter` field it had ignored). The generated `.aip.rs`
  format changes to `use`-free / fully-qualified, and the mount convention changes
  to "include every `.aip.rs` directly in the message module" — the per-file
  submodule and flat re-export are removed. Existing wrapper golden fixtures are
  re-blessed.
- **Footnote to ADR-0009.** The emitted impls read fields by plain borrow
  (`&self.page_token`), not through a **Dynamic message**; the typed-facade /
  dynamic-core split is unaffected. `request_checksum` still reflects.
- **freight-server** (its own follow-up, per the example-server rule): deletes the
  four hand-written impls, sets `pagination=true` + `ordering=true` on the
  `protoc-gen-prost-aip` plugin in `buf.gen.yaml`, adds `aip-pagination` /
  `aip-ordering` as direct dependencies, and gains a committed
  `freight_service.aip.rs`. `parse_page` is unchanged.
- **`MethodType` is unused by this feature** but stays in `aip-codegen` for the
  name/grammar codegen it was ported for (#62).
- **New glossary term.** CONTEXT.md fixes **Request descriptor**, disambiguated
  from the reflection **Descriptor**.
- **Risk to verify in implementation** (asserted from how prost / Rust coherence
  work, not yet proven here): that mounting every `.aip.rs` directly in the message
  module compiles for a **mixed** proto — a wrapper struct and a trait impl side by
  side, both fully-qualified, no name clash — and that fully-qualifying the wrapper
  bodies leaves the roundtrip golden tests green.
