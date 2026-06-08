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
