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
/// Produced by [`transpile_order_by`] from an AIP-132 [`OrderByField`], or built
/// directly (e.g. to append a resource-name tie-break) with [`Order::asc`] /
/// [`Order::desc`].
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

/// Transpile a parsed [`OrderBy`] into SQL `ORDER BY` [items](Order), mapping each
/// field path to its column through the [`Schema`] (the column allowlist) and
/// preserving the field order so a multi-field directive renders its columns in
/// priority order.
///
/// A field path with no column mapping is [`Error::UnknownIdentifier`] — the same
/// gate [`transpile_filter`](crate::transpile_filter) applies to an unmapped
/// identifier. An empty `order_by` yields an empty `Vec`.
pub fn transpile_order_by(order_by: &OrderBy, schema: &Schema) -> Result<Vec<Order>, Error> {
    order_by
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
        .collect()
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
