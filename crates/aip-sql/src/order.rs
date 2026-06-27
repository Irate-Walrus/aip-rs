//! Transpiling an AIP-132 [`OrderBy`] into SQL `ORDER BY` [items](Order), and
//! rendering them to their `col ASC` / `col DESC` spelling.
//!
//! Unlike a [`Filter`](aip_filtering::Filter), an `order_by` carries no
//! attacker-controlled literals: each field path is validated against the column
//! [`Schema`] (the allowlist) and mapped to a fixed column name, and the
//! direction is one of `ASC` / `DESC`. So the `ORDER BY` columns are rendered
//! *directly* (no bound [`Value`](crate::Value)s), the way the comparison
//! operators are — there is nothing to parameterize. [`Query`](crate::Query)
//! assembles these items, together with the `LIMIT` / `OFFSET` page tail, into
//! the full clause tail of a list query.

use aip_filtering::Type;
use aip_ordering::OrderBy;

use crate::schema::Schema;
use crate::Error;

/// The ordered cursor seek columns: each a SQL column paired with its declared
/// [`Type`], in `ORDER BY` clause order with the key tie-break last. Built once by
/// [`transpile_order_by`] and fed to both the cursor build and the cursor decode.
pub type CursorColumns = Vec<(String, Type)>;

/// One `ORDER BY` item: a SQL column and its sort direction.
///
/// Produced by [`transpile_order_by`] from an AIP-132 [`OrderByField`] (which also
/// appends the key-column tie-break itself), or built directly with
/// [`Order::asc`] / [`Order::desc`].
///
/// [`OrderByField`]: aip_ordering::OrderByField
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// The SQL column to sort by — a column name from the [`Schema`], never raw
    /// filter input.
    pub column: String,
    /// Descending if true, ascending otherwise.
    pub desc: bool,
}

impl Order {
    /// An ascending order on `column`.
    pub fn asc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            desc: false,
        }
    }

    /// A descending order on `column`.
    pub fn desc(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            desc: true,
        }
    }
}

/// Transpile a parsed [`OrderBy`] into the cursor seek key and SQL `ORDER BY`
/// [items](Order), mapping each field path to its column through the [`Schema`]
/// (the column allowlist) and preserving field order so a multi-field directive
/// renders its columns in priority order.
///
/// Returns the ordered `(column, Type)` seek list — fed to both the cursor build
/// and the cursor validate (so the column list is derived once) — alongside the
/// [`Order`] items for the SQL clause. A field path with no column mapping is
/// [`Error::UnknownIdentifier`], the same gate
/// [`transpile_filter`](crate::transpile_filter) applies.
///
/// # Always-on key tie-break
///
/// `key_columns` — the resource's key columns, ASC — are appended so the order is
/// total and stable across cursor pages: equal `order_by` keys fall back to the
/// primary key, the exact columns the cursor seeks on. A key column the user
/// already sorts on is not duplicated; an empty `order_by` yields just the key
/// tie-break. Key columns are uniformly text by AIP-122, so each carries
/// [`Type::String`].
pub fn transpile_order_by(
    order_by: &OrderBy,
    schema: &Schema,
    key_columns: &[&str],
) -> Result<(CursorColumns, Vec<Order>), Error> {
    let mut columns: CursorColumns = Vec::new();
    let mut orders: Vec<Order> = Vec::new();

    for field in &order_by.fields {
        let column = schema
            .column(&field.path)
            .ok_or_else(|| Error::UnknownIdentifier(field.path.clone()))?;
        let ty = schema.column_type(column).cloned().unwrap_or(Type::String);
        columns.push((column.to_string(), ty));
        orders.push(Order {
            column: column.to_string(),
            desc: field.desc,
        });
    }

    // Append the key-column tie-break (ASC) so the order is total over exactly the
    // cursor's seek columns; skip a key column the user already sorts on.
    for key in key_columns {
        if orders.iter().any(|order| order.column == *key) {
            continue;
        }
        columns.push(((*key).to_string(), Type::String));
        orders.push(Order::asc(*key));
    }

    Ok((columns, orders))
}

/// Render `ORDER BY` items to their SQL spelling — `col ASC` / `col DESC` joined
/// by `, ` — *without* the leading `ORDER BY`. The column names come from the
/// [`Schema`] allowlist (never raw filter input) and `ASC` / `DESC` are standard
/// SQL identical across engines, so they are written directly rather than bound
/// or per-[`Dialect`](crate::Dialect) spelled. An empty slice renders `""`.
///
/// Internal: callers reach this through [`Query::render`](crate::Query::render),
/// which assembles it with the `LIMIT` / `OFFSET` tail.
pub(crate) fn render_order_by(items: &[Order]) -> String {
    items
        .iter()
        .map(|order| {
            let direction = if order.desc { "DESC" } else { "ASC" };
            format!("{} {direction}", order.column)
        })
        .collect::<Vec<_>>()
        .join(", ")
}
