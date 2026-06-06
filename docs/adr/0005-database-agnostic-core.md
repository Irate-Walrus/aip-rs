# Core stays database-agnostic; SQL adapter deferred

Like `aip-go`, aip-rs v0.1 stops at AIP primitives: `filtering` yields a native
AST, `ordering` an `OrderBy`, `pagination` offsets/cursors — all
database-agnostic. The five core crates depend on no datastore.

A SQL query *generator* (AIP request → parameterized SQL fragment + ordered bound
values), with thin optional `sqlx` execution glue, is planned as a **separate
optional crate** (e.g. `aip-sql` / `aip-sqlx`) built **after** the primitives —
not part of the core, and not forced on users who only parse/validate. We do not
build a full executor/repository/ORM mapping every AIP standard method to the DB.

## Constraints for the future adapter

- **Parameterize, never interpolate.** A filter is attacker-controlled input, so
  the generator must emit placeholders plus a bound-value list; filter literals
  must never be spliced into SQL text.
- **Executor-agnostic core.** The generator emits SQL + binds usable by any
  executor; `sqlx` is one optional integration.
- **Dialects.** Start by hand-rolling one dialect (Postgres). Multi-dialect
  generation via `polyglot` is deferred and, if adopted, kept an internal
  implementation detail rather than public API.

## Consequences

- v0.1 users translate AIP outputs to their datastore themselves, exactly as with
  aip-go.
- The native filter AST (ADR-0003) is the integration point, so it must stay
  ergonomic to walk for a future transpiler.
