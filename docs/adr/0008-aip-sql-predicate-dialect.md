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
