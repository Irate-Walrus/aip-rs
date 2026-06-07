//! The column [`Schema`]: which filter identifiers map to which SQL columns.

use std::collections::HashMap;

/// Maps a filter **Identifier** to the SQL column it addresses, and so acts as
/// the allowlist of what may be filtered.
///
/// The transpiler is handed a `Schema` because [`aip_filtering::check`] discards
/// types — it returns only the untyped `Expr` — so the column mapping (and, in
/// later slices, the recovered column type) lives here rather than on the
/// [`Filter`](aip_filtering::Filter). An identifier absent from the schema is
/// [`Error::UnknownIdentifier`](crate::Error::UnknownIdentifier).
#[derive(Debug, Clone, Default)]
pub struct Schema {
    columns: HashMap<String, String>,
}

impl Schema {
    /// Start building a column schema.
    pub fn builder() -> SchemaBuilder {
        SchemaBuilder::default()
    }

    /// The SQL column a filter identifier maps to, if it is filterable.
    pub(crate) fn column(&self, identifier: &str) -> Option<&str> {
        self.columns.get(identifier).map(String::as_str)
    }
}

/// Builder for a [`Schema`], mirroring `aip-filtering`'s declarations builder.
#[derive(Debug, Default)]
pub struct SchemaBuilder {
    columns: HashMap<String, String>,
}

impl SchemaBuilder {
    /// Map filter `identifier` to SQL `column`. A repeated identifier replaces the
    /// earlier mapping.
    pub fn column(mut self, identifier: &str, column: &str) -> Self {
        self.columns
            .insert(identifier.to_string(), column.to_string());
        self
    }

    /// Finalize the schema.
    pub fn build(self) -> Schema {
        Schema {
            columns: self.columns,
        }
    }
}
