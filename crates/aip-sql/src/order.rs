//! Transpiling an AIP-132 [`OrderBy`] into SQL `ORDER BY` items, and rendering
//! the `ORDER BY` / `LIMIT` / `OFFSET` tail of a list query.
//!
//! Unlike a [`Filter`](aip_filtering::Filter), an `order_by` carries no
//! attacker-controlled literals: each field path is validated against the column
//! [`Schema`] (the allowlist) and mapped to a fixed column name, and the
//! direction is one of `ASC` / `DESC`. So `ORDER BY` columns and the `LIMIT` /
//! `OFFSET` integers are rendered *directly* (no bound [`Value`](crate::Value)s),
//! the way the comparison operators are — there is nothing to parameterize.

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
pub fn render_order_by(items: &[Order]) -> String {
    items
        .iter()
        .map(|order| {
            let direction = if order.desc { "DESC" } else { "ASC" };
            format!("{} {direction}", order.column)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render the `LIMIT` / `OFFSET` tail from a resolved page size and offset.
///
/// Both are non-negative server-resolved integers — the page size after the
/// AIP-158 default/cap, and the offset carried by the AIP-158 offset
/// [`PageToken`] — so they are written as decimal literals rather than bound. A
/// page token's offset is client-forgeable, but as a `u64` it can only ever
/// render as digits, so this honors "parameterize, never interpolate" (ADR-0005 /
/// ADR-0008): there is no free-form text to splice.
///
/// [`PageToken`]: https://docs.rs/aip-pagination
pub fn render_limit_offset(limit: u64, offset: u64) -> String {
    format!("LIMIT {limit} OFFSET {offset}")
}
