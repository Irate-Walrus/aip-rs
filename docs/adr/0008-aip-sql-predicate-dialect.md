# `aip-sql`: a parameterized `Predicate` fragment rendered by a `Dialect`

ADR-0005 deferred the SQL adapter, named `aip-sql` / `aip-sqlx` as a separate
optional crate, and fixed two hard constraints: **parameterize, never
interpolate** (a **Filter** is attacker-controlled input) and **executor-agnostic**
output. This ADR fixes the *shape* of that adapter and supersedes ADR-0005's
"Postgres first" line: the first dialect is **SQLite**, then **Postgres**, with any
engine pluggable.

## Decision

`aip-sql` transpiles the primitives' native ASTs — the **Filter** AST (ADR-0003)
and the **Order by** — into a small, composable, parameterized **Predicate**: a
boolean SQL fragment whose logical structure (`AND`/`OR`/`NOT`) is portable and
whose leaves (a comparison, a `LIKE`, a membership test) are *spelled* by a
**Dialect**. A single render pass turns a `Predicate` into `(sql, Vec<Value>)` —
SQL text plus an ordered list of **Bind values** — assigning every placeholder in
one left-to-right walk and parenthesizing by precedence. The core depends on no
datastore; the consumer binds the **Bind values** to whatever driver it uses.

- `transpile_filter(&Filter, &Schema) -> Predicate` walks the **Filter** AST.
  Because `check()` discards types (it returns `Result<()>`; the `Filter` keeps
  only the untyped `Expr`), the transpiler is handed the **Declarations** / a
  column **Schema** so it can recover enum/map/timestamp typing and map each
  **Identifier** to a column.
- `transpile_order_by(&OrderBy, &Schema) -> Vec<Order>` yields the `ORDER BY`
  items; `LIMIT`/`OFFSET` come from the **Page token** offset and page size.
- `Predicate` is also a public builder (`all` / `any` / `not` / `eq` / `is_null` /
  `raw`, plus `scope_to_parent`) so a server composes the user's **Filter** with
  its own predicates — parent scoping, tenancy, soft delete — through one fragment
  that owns precedence and placeholder numbering.

## Why a `Predicate`, not the alternatives

- **A bare SQL string** cannot safely compose server-side predicates: appending
  `AND delete_time IS NULL` to a user **Filter** like `a OR b` silently binds as
  `a OR (b AND …)` (precedence), and concatenating independently-numbered
  positional params (`$1`, `$2`) collides. The `Predicate` centralizes both.
- **A full typed SQL AST** (the `spansql` analog einride's `spanfiltering`
  emits) is more than is needed here. `spanfiltering` could reuse `spansql`
  because Google already had it; we would be authoring a SQL grammar just to model
  boolean predicates.
- **`polyglot` (`polyglot-sql`)** — the 30+-dialect transpiler ADR-0005 parked —
  models literals **inline** (no first-class bind-parameter node, per its docs),
  which collides head-on with parameterize-never-interpolate; it is also a heavy,
  pre-1.0 dependency. It stays a *possible future optional renderer* behind the
  `Dialect` seam (ADR-0005's "internal, not public"), viable only if it gains a
  parameter node.

## Considered options

- **Emit `(sql, binds)` directly from the Filter walk** (no `Predicate`) —
  simplest, but the server-side composition requirement (scoping/tenancy/soft
  delete) re-introduces the precedence and param-numbering footguns above.
- **Reuse `polyglot`'s AST as the transpile target** — buys 30+ dialects, but
  has no parameter node today, so it cannot honor the core security constraint.
- **Hand-roll a full SQL expression AST** — maximally composable, but a large
  surface to author and maintain for a boolean-predicate use case.

## Constraints & consequences

- Inherits ADR-0005: parameterize never interpolate; executor-agnostic. This
  **supersedes ADR-0005's "Postgres first"** → **SQLite first, then Postgres**;
  further dialects are a `Dialect` impl away.
- The `:` **has operator** is the main per-dialect divergence (substring `LIKE`;
  map-key / list membership) and lives in `Dialect` leaves.
- `scope_to_parent` must **escape** `%` / `_` in the parent prefix and bind it —
  never interpolate.
- Per CLAUDE.md a feature is not done until the example uses it: freight-server
  gains a feature-gated **SQLite-backed storage** (the in-memory store stays the
  default — a minor amendment to ADR-0006) and a `filter` field on
  `ListSitesRequest` / `ListShipmentsRequest`, so `ListSites` / `ListShipments`
  compose scope + soft-delete + **Filter** → SQLite end-to-end.
- v1 is transpiler-only; the `aip-sqlx` execution glue is deferred and optional.

## Amendment (#39): the example defaults to in-memory SQLite

The tracer bullet wired the example's Site store as a **default** in-memory
SQLite database rather than behind a `sqlite` feature, so `cargo run -p
freight-server` proves the filter → `Predicate` → SQLite path with no opt-in.
This refines the "feature-gated storage" line above; the trade-off is that
building the example now needs a C toolchain (rusqlite `bundled`). The core
crates stay datastore-free (ADR-0005), and the slice still ships only `=`/`AND`
over scalar columns with parent scoping in the service layer (`scope_to_parent`
is #43).

## Amendment (#40): full operator grammar and type recovery

The transpiler now lowers the full operator set the checker accepts. Decisions
fixed here, since `check()` discards types and the transpiler re-derives them:

- **Signature.** `transpile_filter(&Filter, &Declarations, &Schema)`. The
  `Schema` maps each **Identifier** to a column (and is the column allowlist);
  the `Declarations` — the same set the filter was checked against — let the
  transpiler recover an operand's type *without re-running the checker*. The one
  case that needs them is telling a bare **enum value** (an `Ident` declared with
  an `Enum` type but absent from the `Schema`) apart from a declared scalar
  column that the caller forgot to map (→ `UnknownIdentifier`).
- **Operators.** `=` `!=` `<` `<=` `>` `>=` lower to a `Predicate::Compare`
  leaf carrying a `CmpOp`; `AND`/`OR`/`NOT` to the existing combinators. A
  comparison is normalized to `column <op> value` — if the column sits on the
  right, the operator is mirrored (`"x" < c` → `c > "x"`). The comparison
  operators are standard SQL, so they are rendered directly, not per-`Dialect`.
- **Enum** comparisons render the value **as its name (a `TEXT` bind)**, not its
  number. This keeps the transpiler reflection-free (no `EnumDescriptor` needed
  to map a name to an integer) and human-readable; the column is expected to
  store the value name, which the example does via prost's `as_str_name()`.
- **Timestamp** literals (a bare RFC3339 string, or one lifted by
  `timestamp(...)`) bind as **text**; the column is expected to store sortable
  RFC3339. **Duration** literals (`duration("3600s")`) bind as their **total
  seconds (`Double`)** so they compare numerically; a non-seconds string is an
  `InvalidDuration` error (the checker accepts any string argument).
- **Member access** into a `map` column (`labels.env`) renders `column ->> ?`
  with the **key bound** (it is filter input, never interpolated). `->>` is
  shared by SQLite and Postgres, so it is not a per-`Dialect` leaf. A dotted path
  the `Schema` declares as one column (`lat_lng.latitude`) instead maps straight
  to that column.

The has operator `:` and a comparison between two columns remain
`Error::Unsupported` (`:` is #41). Per CLAUDE.md the example exercises this: a
`Site.state` enum is added to the demo proto, and `ListSites` runs numeric,
timestamp, and enum filters end-to-end through SQLite.

## Amendment: reached via the umbrella's `aip::sql`

`aip-sql` remains a separately published crate (ADR-0001), but consumers no
longer depend on it directly: the umbrella `aip` crate re-exports it under a
**non-default** `sql` feature as `aip::sql`. This keeps the "not part of the
core" stance of ADR-0005 — the SQL adapter is opt-in, off by default, so a
parse/validate-only user never pulls it in — while removing the last `aip_*`
import from consumer code. The example now enables `aip = { features = ["sql"] }`
and calls `aip::sql::transpile_filter` / `aip::sql::Predicate` /
`aip::sql::Sqlite` rather than the bare `aip_sql` crate.

## Amendment (#41): the has operator `:`

The transpiler now lowers the AIP-160 has operator `:`, the last operator the
checker accepts. Unlike the comparisons, `:` is **the main per-engine divergence**
(foreseen above), so it is a `Dialect` leaf rather than a directly-rendered one.

- **Shape.** A new `Predicate::Has { column, test: HasTest }` leaf carries the
  column and a `HasTest` — `Substring` / `Key` / `Element` / `Present` — one per
  overload the checker accepts (string, `map<string,string>`, `list<string>`,
  timestamp). The transpiler picks the variant from the left operand's declared
  type, recovered from the `Declarations` exactly as #40 recovers enums.
- **Per-`Dialect` spelling.** A new `Dialect::render_has(column, test, next_bind)
  -> (sql, binds)` method spells the leaf; `next_bind` is its first 1-based
  placeholder, so it numbers in step with the shared `render` pass. SQLite spells:
  a substring as `column LIKE ?n ESCAPE '\'` binding `%value%` with the value's
  `LIKE` metacharacters (`%` `_` `\`) escaped — so user input matches literally,
  never as a wildcard, and is never interpolated; map-key and list-element
  presence as `EXISTS (SELECT 1 FROM json_each(column) WHERE key|value = ?n)`;
  and presence (`field:*`) as `column IS NOT NULL` (no bind). The has leaf binds
  as tightly as a comparison, so it negates and composes without redundant parens.
- **Map vs. member access.** `map:key` (this slice, key *presence*) is distinct
  from `map.key` (#40, the *value* at a key via `->>`). A timestamp takes only
  the `*` wildcard, which the checker already enforces, so `Present` binds
  nothing. A comparison between two columns is still `Error::Unsupported`.

Per CLAUDE.md the example exercises this end-to-end: `Site` gains an
`annotations` map and a `tags` list (stored as JSON), and `ListSites` runs
substring, map-key, and list-element has filters through SQLite `json_each`.

## Amendment (#42): ordering + pagination → `ORDER BY` / `LIMIT` / `OFFSET`

The Order by half of the Decision is now implemented, alongside the page tail.

- **Signature.** `transpile_order_by(&OrderBy, &Schema) -> Result<Vec<Order>, Error>`
  maps each AIP-132 `order_by` field path to a column through the *same* `Schema`
  the filter uses (the column allowlist), preserving priority order. A path the
  `Schema` does not map is `Error::UnknownIdentifier`, the same gate the filter
  transpiler applies. An `Order` is just a `{ column, desc }` pair.
- **Rendered directly, not bound.** Unlike a `Filter`, an `order_by` carries no
  attacker-controlled literals: a path is validated against the allowlist and
  mapped to a fixed column name, and the direction is `ASC` / `DESC`. So
  `render_order_by(&[Order]) -> String` writes the columns directly (like the
  comparison operators, not via a `Dialect` leaf), and `render_limit_offset(limit,
  offset) -> String` writes the page size and offset as decimal literals. The
  offset comes from a forgeable page token, but as a `u64` it can only render as
  digits — so "parameterize, never interpolate" still holds; there is nothing
  splice-able to bind.
- **Example pages in SQL.** `ListSites` transpiles the validated `order_by`,
  appends a resource-name tie-break so the order is total and stable across pages,
  and the SQLite store runs `ORDER BY <items> LIMIT <size+1> OFFSET <offset>` — the
  `+1` row signals a further page (the AIP-158 `next_page_token`), replacing the
  old in-memory `sort_sites` + slice. The `sites` table gains `update_time` /
  `longitude` columns so every sortable allow-list path has a column.
- **Parent scoping stays service-side.** Per the #41 split, parent scoping remains
  an in-memory post-filter (`scope_to_parent` is #43). For the demo's
  single-parent listings that post-filter drops nothing, so the SQL page
  boundaries match the previous in-memory path; composing parent scope *into* the
  SQL `WHERE` (so multi-parent paging is exact) lands with `scope_to_parent` in
  #43.

## Amendment (#43): server-side composition and `scope_to_parent`

`Predicate` becomes a public builder so a server folds its own predicates in with
the user's transpiled Filter — the composition the Decision opened with, now
realized.

- **Builders.** `is_null`, `raw`, and `scope_to_parent` join the existing `all` /
  `any` / `not` / `eq`. `scope_to_parent(column, parent)` is an AIP parent scope:
  a `LIKE` prefix keeping the resource names under `parent`, binding
  `escape_like(parent) + "/%"` so the parent's `%` / `_` / `\` match literally and
  the child wildcard is the only one — never interpolated, honoring the cardinal
  rule. The `/` before `%` enforces the segment boundary, so `shippers/acme` does
  not scope `shippers/acme2/...`.
- **Rendered directly, not per-`Dialect`.** `IS NULL` and the `LIKE … ESCAPE '\'`
  scope are standard SQL identical across SQLite and Postgres, so — like the
  comparison operators and the `order_by` tail — they render in the shared pass
  rather than through a `Dialect` leaf. Both are atoms (leaf precedence), so they
  negate and compose without redundant parens.
- **`raw` is the bind-free escape hatch.** A verbatim boolean fragment for a server
  predicate the typed builders don't cover. It carries no placeholders (so the
  single coherent numbering is untouched) and is treated as the loosest-binding
  node, so it is always parenthesized under a combinator — the composition's
  precedence guarantee holds even through the escape hatch.
- **Example composes in SQL.** `ListSites` now transpiles the user filter and folds
  it through `Predicate::all` with `scope_to_parent("name", parent)` and the
  soft-delete `is_null("delete_time")`, dropping the in-memory `has_parent`
  post-filter — so the `LIMIT`/`OFFSET` page boundaries are computed over exactly
  the in-scope, non-deleted rows (multi-parent paging is now exact, superseding the
  #42 service-side note). This unblocks `ListShipments`: it is now SQLite-backed,
  gains a `filter` field, and runs the *same* `scoped_predicate` composition (no
  `order_by`, so it orders by resource name); a minimal `CreateShipment` mirrors
  `CreateSite` to seed it, and the `sites` / `shipments` tables gain a
  `delete_time` column. A tenancy predicate composes identically — a golden test
  pins `scope + eq("tenant_id", …) + is_null + filter` rendering to one fragment
  with left-to-right placeholder numbering — though the freight demo's tenancy
  boundary is the parent scope itself (a shipper owns its sites and shipments).

## Amendment: the unified `Query`

Until now a caller assembled a list query from two unrelated rendering paths: the
**Filter** half (`transpile_filter` → `Predicate` → `Dialect::render` → `(sql,
binds)`) and the **Order by** / page half (`transpile_order_by` → `Vec<Order>`,
then the free functions `render_order_by` + `render_limit_offset` → bare
strings), `format!`-ing the three fragments together at the call site (the
example's `query_page`). That the WHERE and the `ORDER BY` / `LIMIT` rendered
through two different mechanisms was the friction this amendment removes.

- **A `Query` bundles the tail.** A new `Query` holds the WHERE `Predicate`, the
  `ORDER BY` [`Order`]s, and the `LIMIT` / `OFFSET`, built fluently
  (`Query::new().filter(..).order_by(..).limit(..).offset(..)`). One
  `Query::render(&dialect)` pass emits the whole `WHERE … ORDER BY … LIMIT …
  OFFSET` clause tail plus the binds — a single call replacing the hand-stitched
  three. Only the parts that are set render; an empty `Query` renders `("", [])`.
- **Clause tail only — no `SELECT` / `FROM`.** The `Query` owns no head: the
  table and projection name the caller's storage, which an executor-agnostic
  adapter (ADR-0005) has no business spelling. `render` returns no leading or
  trailing space, so the caller writes `format!("SELECT … FROM {table} {tail}")`.
- **WHERE is a pre-composed `Predicate`.** The `Query` does *not* re-transpile a
  filter or re-introduce a raw-filter entry point: it takes the already-composed
  WHERE predicate, so the server-side composition of #43 (parent scope, tenancy,
  soft delete folded in with the user's filter) is unchanged, and the WHERE keeps
  its single coherent placeholder numbering. The WHERE is the only source of
  binds; `render` reuses `Dialect::render` for it, and the `ORDER BY` columns and
  the `u64` `LIMIT` / `OFFSET` integers render directly (no binds), exactly as #42
  established.
- **`render_order_by` / `render_limit_offset` are now internal.** `render_order_by`
  becomes `pub(crate)` (a helper `Query::render` calls) and `render_limit_offset`
  is folded into `Query::render` directly; both leave the public surface. This
  supersedes the #42 line that exposed them as public free functions —
  `Query::render` is the one blessed way to render the tail. `transpile_filter`,
  `transpile_order_by`, `Order`, and the `Predicate` builders are unchanged: they
  remain how a caller *builds* the pieces a `Query` assembles.

### Off-the-shelf builders considered

- **sqlx** — rejected as the wrong layer. sqlx is the async driver / execution
  toolkit, i.e. the `aip-sqlx` execution glue ADR-0005 deferred and kept
  optional. Its `QueryBuilder` is a string-pusher with no expression AST, so it
  models neither precedence nor coherent placeholder numbering — the same "bare
  SQL string" alternative the Decision already rejected — and it would reverse the
  datastore-/executor-agnostic constraint.
- **sea-query** — the only genuine off-the-shelf match (driver-agnostic, an
  `Expr`/`Cond` AST, dialect builders, `(sql, Values)` output). But adopting it
  re-opens this ADR's core `Predicate` decision and adds a heavy pre-1.0 dep to a
  crate ADR-0001 keeps lean, while the has-operator leaves (`LIKE … ESCAPE`,
  `json_each` / `EXISTS`) would still need custom-SQL escape hatches. It buys
  nothing for this unification, which is pure assembly over the existing renderer,
  so the hand-rolled `Predicate` / `Dialect` foundation stands.

Per CLAUDE.md the example uses it: `query_page` now builds one `Query` and drops
the manual `Dialect::render` + `format!` stitching (and the `Dialect` import) for
a single `render(&Sqlite)`.
