# Typed-key resource store: resource names as key-column tuples, Spanner-style

The store flattens a **Resource name** to a single `name TEXT PRIMARY KEY` column
and scopes a list to its parent with `LIKE …/%`. `LIKE`'s `%` spans `/`, so a
**Wildcard** in any but the last position over-matches across **Segment**
boundaries — `shippers/-/sites/oslo` renders `name LIKE 'shippers/%/sites/oslo/%'`,
which also matches `shippers/a/teams/x/sites/oslo/…`. `LIKE` has no portable
single-segment wildcard. The resource-name layer already matches a **Wildcard**
correctly, segment by segment (a `-` matches exactly one **Resource ID**); the
over-match is introduced *only* at the SQL-string flattening boundary. A
SQL-side stopgap — rendering a `-` as `%` (explored in PR #196) — fixes only the
terminal `shippers/-` case freight actually uses and is non-portable
(`GLOB`/`[^/]*` is SQLite-only), so it was closed unmerged in favour of the model
recorded here.

Google/Spanner have no such defect because they model the hierarchy as **separate
typed key columns**, not a flat string. A **Wildcard** at a position is
structurally "do not constrain that column" — the column's equality predicate is
omitted. No string, no `/`, no `%`, so single-segment semantics are inherent and
portable across SQLite *and* Postgres.

## Decision

Decompose each **Resource name** into its **Variables** as typed columns — one
column per **Variable**, in **Pattern** order — and make that key-column tuple the
table's primary key. A child resource's key columns are a superset of its
parent's, so the hierarchy is the column layout (logical interleaving; neither
SQLite nor Postgres interleaves physically, but a composite primary key plus a
foreign key replicates it). The canonical **Resource name** becomes a
presentation projection of the key, reconstructed on read — not the stored
identity. This adopts Spanner's *semantic* model wholesale (key tuples, scoping
by omission, key-tuple ordering, cursor pagination, hierarchy by key prefix);
SQLite remains the underlying engine for the example, free to deviate from
Spanner's *storage layout* only where SQLite gains nothing by matching it.

Freight has no backwards-compatibility constraint — it is the sole consumer of
the surfaces this changes — so superseded machinery is deleted, not preserved
alongside.

### The two deliberate reversals

This reverses two choices the codebase made on purpose; both are accepted.

- **Ordering flips from name-string order to key-tuple order.** The generated
  `*ResourceName` `Ord` and the always-on `name ASC` tie-break sort by the
  canonical name *string*, so `shippers/a-b/…` sorts *before* `shippers/a/…`
  because `-` (`0x2D`) is less than `/` (`0x2F`). The key-tuple order sorts `a`
  before `a-b`. Both are total orders; this adopts the key-tuple one. The
  `Ord` impl (ADR-0011 amendment) and the always-on tie-break (ADR-0008
  amendment) change accordingly, and the test that locks the string order is
  inverted.
- **Pagination flips from offset to cursor.** The current store pages with
  `LIMIT size+1 OFFSET …` and an **Offset page token**. The typed-key store pages
  with a **Cursor page token** carrying the last row's ordered values and a
  `(key columns) > (…)` seek. Strictly better — stable under concurrent inserts,
  reads `n` rows for page `n` rather than `k+n` — at the cost of random-access
  "jump to page 47", which AIP-158 does not expose anyway.

### Drop the `name` column (Q2)

The primary key *is* the typed key columns; there is no `name` column. The
protobuf `name` field is reconstructed on read from the row's key columns plus the
resource's **Pattern** — one `format!` adjacent to where the message is assembled
from its columns, via the generated wrapper's `Display`. Spanner has no `name`
column and SQLite mirrors that.

Rejected: keeping `name` as a redundant stored column (a drift class of bugs
between it and the key columns); a SQLite `GENERATED VIRTUAL` column (couples the
DDL to the URI **Pattern** and has weaker Postgres support).

### Row layout: column-per-field, no BLOB (Q7)

Freight stores each scalar field as its own typed column — no `data BLOB` of the
encoded message. Small repeated and map fields (`tags`, `annotations`) are JSON
columns. The read path assembles the message from columns rather than decoding a
blob, so every stored field is a first-class, indexable column. For high
cardinality, normalized side tables are the production-correct layout; the ADR
records that as the recommendation and freight stays with JSON columns because the
demo's repeated/map fields are small.

### Hierarchy: foreign key with cascade, hard-delete only (Q8)

Each child table declares
`FOREIGN KEY (parent key columns) REFERENCES parent (parent key columns) ON DELETE
CASCADE`, so deleting a parent row removes its descendants in one statement. The
SQLite connection pool sets `PRAGMA foreign_keys = ON` — a SQLite-specific gotcha,
since SQLite leaves foreign keys *off* per connection by default while Postgres
enforces them.

The cascade fires on **hard delete only**. Soft delete (`delete_time IS NOT NULL`)
stays a handler-layer policy, not a schema concern: soft-deleting a **Shipper**
does *not* soft-delete its **Sites** and **Shipments**. Conflating the two would
entangle the AIP-135 undelete contract — an undeleted parent would need to know
which descendants it had cascaded, state the schema does not carry. Freight tests
that hard-delete a **Shipper** without first clearing its **Sites** now cascade
rather than leave orphans; that is the intended new behaviour.

### Cursor page-token wire format (Q9)

The **Page token** moves from an **Offset page token** to a **Cursor page
token**. The cursor is a *self-describing* ordered list of `(column, value)`
entries — one entry per `ORDER BY` column in clause order, ending with the
always-on key-column tie-break. The wire payload, postcard-encoded behind the
ADR-0004 version byte:

```
PageToken {
    cursor: Vec<CursorEntry>,   // self-describing, in ORDER BY clause order
    request_checksum: u32,      // role unchanged from the offset token
}
CursorEntry { column: String, value: CursorValue }
CursorValue = Bool | Int(i64) | Double(f64) | Text(String) | Bytes(Vec<u8>)
```

Decode validates, in this order, and any failure is `INVALID_ARGUMENT`
(AIP-158):

1. The ADR-0004 version byte equals `2` (a `1` offset token is cleanly rejected;
   the client restarts pagination).
2. The postcard payload parses.
3. `request_checksum` matches the CRC32-IEEE over the prost-encoded request with
   the pagination fields cleared (existing machinery, unchanged) — the guard that
   a mid-pagination filter/order change is detected.
4. The cursor's column list equals the resolved `ORDER BY` column list, position
   by position.
5. Each entry's `CursorValue` variant matches the **Schema**-allowlisted **Type**
   of its declared column.

The next page's token is minted from the last row of the current page: the
ordered values of that row, tagged with their columns. The seek itself is the
`Predicate::tuple_gt` over the same column list (ADR-0008 amendment).

Self-describing is a deliberate choice over a positional `Vec<CursorValue>`. The
column list is technically reconstructable — the request shape is locked by
`request_checksum`, so the `ORDER BY` is too — but carrying column names makes a
token legible at the debug layer and turns the column-list match (step 4) into an
explicit cross-check rather than an assumption. The redundancy with the checksum
is accepted in exchange.

### Cursor values live in `aip-pagination` (Item 1)

`CursorValue` is defined in `aip-pagination`, preserving its leaf-crate status
(it must not depend on `aip-sql`). Its variants are
`Bool | Int(i64) | Double(f64) | Text(String) | Bytes(Vec<u8>)`; null is
forbidden in a cursor entry (a sort over a nullable column would make the seek
ambiguous). **Timestamps** ride in `Text` as sortable RFC3339; proto **enums**
ride in `Text` as their value name, matching how the store already sorts enums by
name (ADR-0008). A handler converts a `CursorValue` to an `aip_sql::Value` at the
freight boundary, the one place that depends on both crates.

### Indexes (Item 6)

One covering index per resource, for the common case only. `sites` gets
`(shipper, display_name, site)` — the `ListSites` `order_by` on `display_name`
followed by the key-column tie-break, the same index an offset scan would want.
`shippers` and `shipments` page on the primary key alone and need no extra index.
Codegen emits no DDL; freight authors the index by hand. Partial indexes for
wildcard-scoped or soft-delete-filtered lists are noted as production tuning and
skipped in the demo. Keyset paging is strictly faster than offset at any
non-trivial depth — offset reads `k+n` rows to return page `n`, keyset reads `n` —
and is same-or-better on page one.

### Batch reads (Item 8)

`BatchGetSites` graduates from `Unimplemented` to a real handler: `N` typed
`get_site` primary-key lookups, no wrapping transaction, with the AIP-231
whole-batch-`NOT_FOUND` default (any missing **Site** fails the batch). AIP-233
atomicity for `BatchCreateShippers` — a multi-row create under one **Operation** —
stays deferred; combining an LRO with SQLite's single-writer model is a tradeoff
out of scope here.

### Read and write path (Item 9)

The read path assembles the response message from its columns and reconstructs
the `name` via the generated wrapper — `<TypedName>::from_parts(…).to_string()` —
adjacent to the `SELECT`. The write path takes the typed name —
`put_*(name: &TypedName, msg)` — and decomposes it into the key-column binds. Key
columns are named verbatim after their **Variables** (`shipper`, `site`), so the
column name, the **Pattern** variable, and the wrapper accessor all read the same.

## Out of scope

- **`ResourceDescriptor` runtime metadata** stays unextended. The freight
  handlers reach the typed wrapper consts (`KEY_COLUMNS`, `pattern()`) directly
  and need no descriptor-level key metadata; that is future work, warranted only
  if a generic, type-erased consumer appears.
- **Multi-pattern resources** are deferred. Freight is single-**Pattern** per
  resource. The expected shape: one table per resource whose columns are the union
  of all patterns' **Variables**, with the `ORDER BY` and scope adapting per
  pattern at the handler.

## Considered Options

- **Do nothing — document the limitation.** Leaves the non-terminal **Wildcard**
  over-match latent in the generic library. Acceptable for freight's terminal-only
  usage, not for the SDK.
- **Per-dialect `GLOB`** (`[^/]*` for a single segment). Fixes the over-match on
  SQLite only; not portable to Postgres, so it fails the executor-agnostic line.
- **Key columns for scoping only** — add typed key columns for the scope predicate
  but keep `name` as the identity and the sort key, page with an offset token. The
  pragmatic midpoint: it fixes the over-match without reversing ordering or
  pagination. Rejected in favour of the purist model — two coexisting orderings
  and an offset token that is strictly worse than a cursor are not worth keeping
  once the typed columns exist anyway.
- **Keep a `data BLOB`** alongside the key columns. Rejected for column-per-field
  (above): a blob hides every field from the index and forces a decode on every
  read.

## Consequences

- The name-string ordering and offset pagination are reversed; the
  `ord_follows_string_order_not_the_variable_tuple` test is renamed and inverted,
  and the offset-token tests give way to cursor-token tests.
- `Predicate::scope_to_parent` and its `LIKE … ESCAPE` parent-scope rendering are
  removed (ADR-0008 amendment); scoping is the handler composing `Predicate::eq`
  per concrete **Variable** and omitting per **Wildcard**. The AIP-159 SQL-side
  wildcard stopgap is superseded before it ever lands.
- freight's `sites` and `shipments` tables drop `name`, gain typed key columns, a
  foreign key with cascade, and a covering index; `PRAGMA foreign_keys = ON` is set
  on the pool. `cargo run -p freight-server` and the README `grpcurl` flows keep
  working, and the README status table is refreshed.
- This change is recorded across one new ADR and five in-file amendments to the
  surfaces each crate exposes:
  - **ADR-0002** — `Pattern::match_with_wildcards`, which yields the per-**Variable**
    bindings (`Some` concrete, `None` wildcard) a handler scopes from.
  - **ADR-0004** — the page-token version byte `1 → 2` and the offset-to-cursor
    payload swap, with `CursorValue`.
  - **ADR-0006** — freight's typed-key schema, the foreign key and `PRAGMA`, and
    `BatchGetSites`.
  - **ADR-0008** — deleting `scope_to_parent`, adding `Predicate::tuple_gt` and
    `Schema::column_type`, and broadening `transpile_order_by` to return the
    ordered column list with a caller-supplied key-column tie-break.
  - **ADR-0011** — the `*ResourceName` wrapper as the key: flipped `Ord`,
    `KEY_COLUMNS` / `key_values` / `pattern()`, retained cached name, inherent
    items only.
