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

use aip_ordering::OrderBy;

use crate::schema::Schema;
use crate::Error;

/// One `ORDER BY` item: a SQL column and its sort direction.
///
/// Produced by [`transpile_order_by`] from an AIP-132 [`OrderByField`] (which also
/// appends the resource-name tie-break itself), or built directly with
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

/// The resource-name column every transpile appends as a tie-break, so the order
/// is total and stable across pages (AIP-122 names the resource `name`). Skipped
/// only when the user's `order_by` already sorts on this column.
const TIE_BREAK_COLUMN: &str = "name";

/// Transpile a parsed [`OrderBy`] into SQL `ORDER BY` [items](Order), mapping each
/// field path to its column through the [`Schema`] (the column allowlist) and
/// preserving the field order so a multi-field directive renders its columns in
/// priority order.
///
/// A field path with no column mapping is [`Error::UnknownIdentifier`] — the same
/// gate [`transpile_filter`](crate::transpile_filter) applies to an unmapped
/// identifier.
///
/// # Always-on tie-break
///
/// A trailing `name ASC` is **always appended** so the order is total and stable
/// across offset pages — equal `order_by` keys fall back to a fixed resource-name
/// order (AIP-122), without which two rows sharing a key could swap places between
/// page boundaries. The append is skipped only when the user's `order_by` already
/// sorts on the `name` column (in either direction), so it is never duplicated or
/// overridden. An *empty* `order_by` therefore yields `[name ASC]`, not an empty
/// `Vec`, so a consumer with no `order_by`
/// (`transpile_order_by(&OrderBy::default(), …)`) gets the stable name order for
/// free. The `name` column is taken literally — the resource-name column by AIP
/// convention — not looked up in the [`Schema`].
pub fn transpile_order_by(order_by: &OrderBy, schema: &Schema) -> Result<Vec<Order>, Error> {
    let mut orders = order_by
        .fields
        .iter()
        .map(|field| {
            let column = schema
                .column(&field.path)
                .ok_or_else(|| Error::UnknownIdentifier(field.path.clone()))?;
            Ok(Order {
                column: column.to_string(),
                desc: field.desc,
            })
        })
        .collect::<Result<Vec<Order>, Error>>()?;

    // Append the resource-name tie-break unless the user already ordered on it,
    // so the result is a total order with no duplicate `name` term.
    if !orders.iter().any(|order| order.column == TIE_BREAK_COLUMN) {
        orders.push(Order::asc(TIE_BREAK_COLUMN));
    }
    Ok(orders)
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
