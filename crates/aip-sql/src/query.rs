//! The [`Query`] — the WHERE [`Predicate`], the `ORDER BY` [items](Order), and
//! the `LIMIT` / `OFFSET` page tail bundled into one value, rendered to a single
//! `(sql, binds)` clause tail.
//!
//! A list query is built from two halves that used to render through two
//! different mechanisms: the **Filter** lowers to a [`Predicate`] rendered by a
//! [`Dialect`] (with bound [`Value`]s), while the **Order by** and the page
//! offset/size render as direct, bind-free text. A `Query` reunites them: one
//! [`render`](Query::render) pass emits `WHERE … ORDER BY … LIMIT … OFFSET` plus
//! the ordered binds the WHERE produced, so the caller makes one call instead of
//! stitching three fragments together by hand.
//!
//! It deliberately owns no `SELECT` / `FROM`: those name the caller's table and
//! projection, which an executor-agnostic adapter has no business spelling
//! (ADR-0005 / ADR-0008). The caller writes the head and interpolates the tail —
//! `format!("SELECT … FROM {table} {tail}")`. The WHERE is a *pre-composed*
//! [`Predicate`], so a server still folds its own predicates — parent scope,
//! tenancy, soft delete — into it before handing it over (ADR-0008 #43); the
//! `Query` only assembles the already-composed pieces.

use crate::order::render_order_by;
use crate::predicate::{Predicate, Value};
use crate::{Dialect, Order};

/// A list query's clause tail: an optional WHERE [`Predicate`], the `ORDER BY`
/// [items](Order), and an optional `LIMIT` / `OFFSET`.
///
/// Built fluently from the pieces the transpilers produce —
/// [`transpile_filter`](crate::transpile_filter) for the WHERE,
/// [`transpile_order_by`](crate::transpile_order_by) for the order — then
/// rendered to one `(sql, binds)` with [`render`](Query::render):
///
/// ```
/// use aip_sql::{Order, Predicate, Query, Sqlite, Value};
///
/// let (tail, binds) = Query::new()
///     .filter(Predicate::eq("region", Value::Text("west".into())))
///     .order_by([Order::asc("display_name"), Order::asc("name")])
///     .limit(51)
///     .offset(100)
///     .render(&Sqlite);
///
/// assert_eq!(
///     tail,
///     "WHERE region = ?1 ORDER BY display_name ASC, name ASC LIMIT 51 OFFSET 100",
/// );
/// assert_eq!(binds, vec![Value::Text("west".into())]);
/// let sql = format!("SELECT data FROM sites {tail}");
/// # let _ = sql;
/// ```
#[derive(Debug, Clone, Default)]
pub struct Query {
    filter: Option<Predicate>,
    order: Vec<Order>,
    limit: Option<u64>,
    offset: Option<u64>,
}

impl Query {
    /// An empty query — no `WHERE`, no `ORDER BY`, no page tail. It renders to
    /// `("", [])`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the WHERE [`Predicate`]. This is the *composed* predicate — fold any
    /// server-side scoping / tenancy / soft-delete in with the user's transpiled
    /// filter through [`Predicate::all`] *before* setting it, so the whole WHERE
    /// shares one coherent placeholder numbering (ADR-0008 #43). A `Query` with no
    /// filter set emits no `WHERE` clause.
    pub fn filter(mut self, predicate: Predicate) -> Self {
        self.filter = Some(predicate);
        self
    }

    /// Set the `ORDER BY` [items](Order), in priority order — typically the output
    /// of [`transpile_order_by`](crate::transpile_order_by), with a resource-name
    /// tie-break appended so the order is total. An empty sequence emits no `ORDER
    /// BY` clause.
    pub fn order_by(mut self, items: impl IntoIterator<Item = Order>) -> Self {
        self.order = items.into_iter().collect();
        self
    }

    /// Set the `LIMIT` — a server-resolved page size (the AIP-158 default/cap,
    /// commonly `size + 1` to detect a further page).
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the `OFFSET` — the AIP-158 offset page token's position. SQLite spells
    /// `OFFSET` only alongside a `LIMIT`, so pair it with [`limit`](Query::limit).
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Render the clause tail — `WHERE … ORDER BY … LIMIT … OFFSET`, with only the
    /// parts that are set — plus the ordered bind [`Value`]s, in one pass. The
    /// returned SQL carries no `SELECT` / `FROM` head and no leading or trailing
    /// space, so the caller writes `format!("SELECT … FROM {table} {tail}")`.
    ///
    /// The WHERE is the **only** source of binds: it reuses [`Dialect::render`],
    /// so its single left-to-right placeholder numbering and precedence
    /// parenthesization are unchanged. The `ORDER BY` columns (from the
    /// [`Schema`](crate::Schema) allowlist) and the `LIMIT` / `OFFSET` integers
    /// carry no attacker-controlled text — a `u64` can only render as digits — so
    /// they are written directly, honoring "parameterize, never interpolate"
    /// (ADR-0005 / ADR-0008 #42): there is nothing splice-able to bind.
    pub fn render<D: Dialect>(&self, dialect: &D) -> (String, Vec<Value>) {
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Value> = Vec::new();

        if let Some(filter) = &self.filter {
            let (where_sql, where_binds) = dialect.render(filter);
            clauses.push(format!("WHERE {where_sql}"));
            binds = where_binds;
        }
        if !self.order.is_empty() {
            clauses.push(format!("ORDER BY {}", render_order_by(&self.order)));
        }
        // `LIMIT` / `OFFSET` are server-resolved `u64`s, rendered as decimal
        // literals rather than bound — they can only ever spell as digits.
        if let Some(limit) = self.limit {
            clauses.push(format!("LIMIT {limit}"));
        }
        if let Some(offset) = self.offset {
            clauses.push(format!("OFFSET {offset}"));
        }

        (clauses.join(" "), binds)
    }
}
