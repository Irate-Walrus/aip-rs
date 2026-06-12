# `aip-sql`: a parameterized `Predicate` fragment rendered by a `Dialect`

ADR-0005 deferred the SQL adapter, named `aip-sql` / `aip-sqlx` as a separate
optional crate, and fixed two hard constraints: **parameterize, never
interpolate** (a **Filter** is attacker-controlled input) and
**executor-agnostic** output. This ADR fixes the shape of that adapter and
supersedes ADR-0005's "Postgres first": the first dialect is **SQLite**, then
Postgres, with any further engine a `Dialect` impl away.

## Decision

`aip-sql` transpiles the primitives' native ASTs — the **Filter** AST (ADR-0003)
and the **Order by** — into a small, composable, parameterized **Predicate**: a
boolean SQL fragment whose logical structure (`AND`/`OR`/`NOT`) is portable and
whose engine-divergent leaves are spelled by a **Dialect**. A single render pass
turns a `Predicate` into `(sql, Vec<Value>)` — SQL text plus an ordered list of
**Bind values** — assigning every placeholder in one left-to-right walk and
parenthesizing by precedence. The core depends on no datastore; the consumer
binds the values to whatever driver it uses.

- **`transpile_filter(&Filter, &Declarations, &Schema) -> Predicate`** walks the
  **Filter** AST. Because `check()` discards types, the transpiler is handed the
  same **Declarations** the filter was checked against — to recover enum /
  timestamp / map typing without re-running the checker — and a column
  **Schema** mapping each **Identifier** to a column, which doubles as the
  column allowlist.
- **`transpile_order_by(&OrderBy, &Schema) -> Vec<Order>`** maps each AIP-132
  `order_by` path to a column through the *same* `Schema`, preserving priority
  order; an unmapped path is `Error::UnknownIdentifier`, the same gate the
  filter transpiler applies. An `Order` is a `{ column, desc }` pair.
- **`Predicate` is a public builder** — `all` / `any` / `not` / `eq` /
  `is_null` / `raw`, plus `scope_to_parent` — so a server folds its own
  predicates (parent scoping, tenancy, soft delete) in with the user's
  transpiled Filter through one fragment that owns precedence and placeholder
  numbering. `scope_to_parent(column, parent)` escapes the parent's `%`/`_`/`\`
  and **binds** it with a `/%` suffix, so the segment boundary holds
  (`shippers/acme` does not scope `shippers/acme2/...`) and the child wildcard
  is the only one. `raw` is the bind-free escape hatch for predicates the typed
  builders don't cover; it is treated as loosest-binding, so it is always
  parenthesized under a combinator.
- **A `Query` bundles the tail.** It holds the WHERE `Predicate`, the `ORDER BY`
  `Order`s, and the `LIMIT` / `OFFSET`, built fluently
  (`Query::new().filter(..).order_by(..).limit(..).offset(..)`); one
  `Query::render(&dialect)` emits the whole `WHERE … ORDER BY … LIMIT … OFFSET`
  clause tail plus the binds — the one blessed way to render it. The `Query`
  owns no `SELECT` / `FROM`: the table and projection name the caller's
  storage, which an executor-agnostic adapter has no business spelling. The
  WHERE is the only source of binds — an `order_by` carries no
  attacker-controlled literals (allowlisted columns plus `ASC`/`DESC`), and the
  `u64` limit/offset can only render as digits — so the tail renders directly
  and "parameterize, never interpolate" still holds.

## Lowering decisions

- **Comparisons** (`=` `!=` `<` `<=` `>` `>=`) lower to a `Compare` leaf,
  normalized to `column <op> value` (the operator is mirrored when the column
  sits on the right). Standard SQL, rendered directly rather than per-`Dialect`
  — likewise `IS NULL`, the `LIKE … ESCAPE` parent scope, and the `ORDER BY`
  tail, all identical across SQLite and Postgres.
- **Enum** comparisons bind the value **name** (`TEXT`), not its number —
  reflection-free (no `EnumDescriptor`) and human-readable; the column stores
  the name (prost's `as_str_name()`).
- **Timestamp** literals bind as text over sortable-RFC3339 columns;
  **duration** literals bind as total seconds (`Double`); a non-seconds string
  is `InvalidDuration`.
- **Member access** into a `map` column (`labels.env`) renders `column ->> ?`
  with the key **bound**; `->>` is shared by SQLite and Postgres. A dotted path
  the `Schema` declares as one column (`lat_lng.latitude`) maps straight to it.
- **The has operator `:`** is the main per-engine divergence, so it is the
  `Dialect` leaf: `Predicate::Has { column, test }` with a `HasTest` per checker
  overload, spelled by `Dialect::render_has`. SQLite spells a `Substring` as
  `column LIKE ?n ESCAPE '\'` binding `%value%` with the `LIKE` metacharacters
  escaped (user input matches literally, never as a wildcard); map-`Key` /
  list-`Element` presence as `EXISTS (SELECT 1 FROM json_each(column) …)`; and
  `Present` (`field:*`) as `column IS NOT NULL` (no bind). Note `map:key` (key
  presence) is distinct from `map.key` (the value at a key, above).
- A comparison between two columns is `Error::Unsupported`.

## Why a `Predicate`, not the alternatives

- **A bare SQL string** cannot safely compose server-side predicates: appending
  `AND delete_time IS NULL` to a user filter `a OR b` silently binds as
  `a OR (b AND …)`, and concatenating independently-numbered placeholders
  collides. The `Predicate` centralizes precedence and numbering. (sqlx's
  `QueryBuilder` is this alternative in library form — and the wrong layer
  besides: it is the execution glue ADR-0005 keeps deferred and optional.)
- **A full typed SQL AST** (the `spansql` analog einride's `spanfiltering`
  emits) is more than boolean predicates need; we would be authoring a SQL
  grammar Google already had.
- **`polyglot`** (the 30+-dialect transpiler ADR-0005 parked) models literals
  inline — no bind-parameter node — colliding head-on with
  parameterize-never-interpolate, and is a heavy pre-1.0 dependency. It stays a
  possible future renderer behind the `Dialect` seam, viable only if it gains a
  parameter node.
- **sea-query** is the one genuine off-the-shelf match (driver-agnostic
  `Expr`/`Cond` AST, dialect builders, `(sql, Values)` output), but adopting it
  re-opens this decision, adds another heavy pre-1.0 dependency to a crate
  ADR-0001 keeps lean, and the has-operator leaves would still need custom-SQL
  escape hatches.

## Constraints & consequences

- Inherits ADR-0005: parameterize never interpolate; executor-agnostic. v1 is
  transpiler-only; the `aip-sqlx` execution glue stays deferred and optional.
- Consumers reach the crate through the umbrella's **non-default `sql`
  feature** (`aip::sql`), keeping ADR-0005's "not part of the core" stance: the
  adapter is opt-in, and a parse/validate-only user never pulls it in.
- Per CLAUDE.md the example proves the path **by default**: freight-server's
  Site and Shipment stores are in-memory SQLite (rusqlite `bundled` — building
  the example needs a C toolchain; the core crates stay datastore-free, a
  narrow amendment to ADR-0006). `ListSites` / `ListShipments` fold
  `scope_to_parent` + the soft-delete `is_null("delete_time")` + the user's
  transpiled **Filter** through `Predicate::all`, transpile the validated
  `order_by` with a resource-name tie-break (so the order is total and stable
  across pages), and page with `LIMIT size+1 OFFSET …` — the `+1` row signals
  the AIP-158 `next_page_token`, and page boundaries are computed over exactly
  the in-scope, non-deleted rows. A golden test pins scope + a tenancy `eq` +
  soft delete + filter rendering to one fragment with left-to-right placeholder
  numbering. The example's `query_page` builds one `Query` and calls a single
  `render(&Sqlite)`.

## Amendment (issue #156): one source of truth — derive the `Schema`, always tie-break

The original design left a consumer wiring one filterable, sortable resource
maintaining **three parallel lists that must agree**: the filter `Declarations`
field set, the `Schema` column map, and a hand-spelled sortable-paths list fed to
`OrderBy::validate_for_paths`. The freight example needed two drift-guard tests
(`sortable_site_paths_resolve_on_the_site_descriptor`,
`sortable_site_paths_map_to_columns_in_the_schema`) purely to hold its own lists
together — a drift-guard test in consumer code is the API telling us the surface
is split too fine.

**Decision:** reduce to one declared source plus explicit overrides, and own the
pagination tie-break inside the transpiler.

- **`Schema::for_declarations(&Declarations)`** derives the column map from the
  same declarations the filter is checked against. Each declared **field path**
  becomes a column whose name defaults to the path; the enum *value* names are
  excluded. `Declarations` grew a `field_paths()` accessor for this, and now
  tracks which idents are declared field paths — `enum_ident`'s value-name inserts
  (`ACTIVE`, …) are recorded internally as *not* field paths, since they carry the
  same `Enum` type as their field and no type test could separate them.
- **The sortable set is type-driven, derived automatically.** A column is sortable
  iff its declared `Type` is `String` / `Int` / `Uint` / `Double` / `Bool` /
  `Timestamp` — the scalars a SQL `ORDER BY` totally orders. An `Enum`, map, list,
  or `Duration` column is filter-only, so a bare `order_by: state` stays rejected
  (matching prior freight behavior). No sortable list lives in consumer code.
- **Two overrides cover what the rule can't.** `SchemaBuilder::column(path, col)`
  renames a column whose SQL name differs from its declared path (a nested path
  flattening, `lat_lng.latitude` → `latitude`); `SchemaBuilder::sort_only(path,
  col)` adds a sortable column with no filter declaration (`update_time`), which is
  also the escape hatch for making a filter-only column sortable.
- **Validation wiring** stays the existing two-gate split, fed from the one
  source: `Schema::sortable_paths()` feeds `OrderBy::validate_for_paths`, so a bad
  user sort path is still `INVALID_ARGUMENT` (via `aip-ordering`), while a column
  missing from the `Schema` stays the `UnknownIdentifier` → `INTERNAL` drift
  signal. `transpile_order_by` is *not* the sortable gate — it maps any schema
  column to its name; `validate_for_paths` upstream is the gate.
- **The resource-name tie-break is always-on inside `transpile_order_by`.** It
  appends a literal `name ASC` (AIP-122 names the resource `name`) so the order is
  total and stable across offset pages, unless the user's `order_by` already sorts
  on `name` in either direction. An empty `order_by` therefore transpiles to
  `[name ASC]`, not an empty `Vec`. This removes the hand-written
  `order.push(Order::asc("name"))` every stable-pagination consumer appended.

**Consequences.** The `aip-sql` `transpile_order_by` golden/unit tests gain a
trailing `name ASC` — an intended contract change. The freight example is rewired
onto `for_declarations` + overrides: `SORTABLE_SITE_PATHS` is deleted, the manual
tie-break push and `ListShipments`'s `[Order::asc("name")]` literal are gone, and
the two drift-guard tests — redundant by construction now that the schema *is* the
derivation — are removed. `Schema::builder()` survives for descriptor-less
consumers; its hand-built columns default to filterable + sortable, since with no
declared types there is nothing to drive the sortable rule.
