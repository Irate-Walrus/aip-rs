//! aip-sql: transpile an AIP-160 [`Filter`] into a parameterized, dialect-rendered
//! SQL [`Predicate`].
//!
//! The native Filter AST (ADR-0003) is the integration point: [`transpile_filter`]
//! walks it into a small, composable boolean [`Predicate`] whose logical structure
//! (`AND`/`OR`/`NOT`) is portable and whose leaves are *spelled* by a [`Dialect`].
//! A single [`Dialect::render`] pass turns a `Predicate` into `(sql, Vec<Value>)` —
//! SQL text plus an ordered list of [bound `Value`s](Value) — numbering every
//! placeholder left-to-right and parenthesizing by precedence.
//!
//! The cardinal rule (ADR-0005 / ADR-0008): **parameterize, never interpolate.**
//! A filter is attacker-controlled, so every literal becomes a bound [`Value`],
//! never spliced into SQL text. This crate depends on no datastore — the caller
//! binds the values to whatever driver it uses.
//!
//! This is the tracer-bullet slice: [`transpile_filter`] supports only `=` and
//! `AND` over scalar columns. See `docs/adr/0008-aip-sql-predicate-dialect.md`.

mod dialect;
mod predicate;
mod schema;
mod transpile;

pub use dialect::{Dialect, Sqlite};
pub use predicate::{Predicate, Value};
pub use schema::Schema;
pub use transpile::transpile_filter;

/// Errors produced when transpiling a [`Filter`](aip_filtering::Filter) into a
/// [`Predicate`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filter construct this slice's transpiler does not handle yet (anything
    /// beyond `=` and `AND` over scalar columns).
    #[error("unsupported filter construct: {0}")]
    Unsupported(String),
    /// A filter identifier with no column mapping in the [`Schema`].
    #[error("filter identifier `{0}` is not a filterable column")]
    UnknownIdentifier(String),
}
