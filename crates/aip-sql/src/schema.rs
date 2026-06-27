//! The column [`Schema`]: which filter identifiers map to which SQL columns, and
//! which of those columns an `order_by` may sort on.

use std::collections::{HashMap, HashSet};

use aip_filtering::{Declarations, Type};

/// Maps a filter **Identifier** to the SQL column it addresses (so acts as the
/// column allowlist), and records which columns are **sortable** (so doubles as
/// the `order_by` allowlist via [`sortable_paths`](Self::sortable_paths)).
///
/// The transpiler is handed a `Schema` because [`aip_filtering::check`] discards
/// types — it returns only the untyped `Expr` — so the column mapping (and the
/// recovered column type) lives here rather than on the
/// [`Filter`](aip_filtering::Filter). An identifier absent from the schema is
/// [`Error::UnknownIdentifier`](crate::Error::UnknownIdentifier).
///
/// # One source of truth
///
/// Build it from the same [`Declarations`] the filter is checked against with
/// [`for_declarations`](Self::for_declarations): the column map is *derived* from
/// the declared field paths, and a column is sortable iff its declared [`Type`]
/// is a scalar order admits (string / number / bool / timestamp — see
/// [`for_declarations`](Self::for_declarations)). That collapses what used to be
/// three hand-kept lists (the filter declarations, this column map, and a
/// separate sortable-paths list) into one. Overrides handle the cases the rule
/// can't: [`column`](SchemaBuilder::column) renames a flattened nested path
/// (`lat_lng.latitude` → `latitude`), [`sort_only`](SchemaBuilder::sort_only)
/// adds a sortable column with no filter declaration (`update_time`).
///
/// [`builder`](Self::builder) stays for descriptor-less consumers; its columns
/// are filterable and sortable by default, since with no declared [`Type`]s there
/// is nothing to drive the sortable rule.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    /// Filter identifier / `order_by` path → SQL column. The column allowlist.
    columns: HashMap<String, String>,
    /// The subset of [`columns`](Self::columns) keys an `order_by` may sort on.
    sortable: HashSet<String>,
    /// SQL column → its declared [`Type`], for columns derived from
    /// [`Declarations`]. Feeds the cursor page token's per-column type check.
    types: HashMap<String, Type>,
}

impl Schema {
    /// Start building a column schema by hand.
    ///
    /// Each [`column`](SchemaBuilder::column) added this way is filterable *and*
    /// sortable: a descriptor-less consumer has no declared [`Type`]s to drive the
    /// sortable rule, so the conservative default is to allow both. Reach for
    /// [`for_declarations`](Self::for_declarations) instead when the filter
    /// [`Declarations`] already exist — it derives the columns and applies the
    /// type-driven sortable rule for you.
    pub fn builder() -> SchemaBuilder {
        SchemaBuilder::default()
    }

    /// Derive a column schema from the [`Declarations`] a filter is checked
    /// against, so one declaration list feeds both the type-checker and the
    /// transpiler.
    ///
    /// Each declared **field path** (see [`Declarations::field_paths`] — the enum
    /// *value* names are excluded) becomes a column whose name **defaults to the
    /// path**, and is marked **sortable** iff its declared [`Type`] is one a SQL
    /// `ORDER BY` totally orders: [`String`](Type::String) / [`Int`](Type::Int) /
    /// [`Uint`](Type::Uint) / [`Double`](Type::Double) / [`Bool`](Type::Bool) /
    /// [`Timestamp`](Type::Timestamp). An [`Enum`](Type::Enum), map, list, or
    /// [`Duration`](Type::Duration) column is filter-only — not sortable — which
    /// is why a bare `order_by: state` stays rejected (its escape hatch is an
    /// explicit [`sort_only`](SchemaBuilder::sort_only)).
    ///
    /// Returns a [`SchemaBuilder`] so the cases the rule can't express are added
    /// fluently before [`build`](SchemaBuilder::build):
    ///
    /// - [`column(path, col)`](SchemaBuilder::column) renames a column whose name
    ///   differs from the declared path — typically a nested path flattening to a
    ///   single column, `lat_lng.latitude` → `latitude`.
    /// - [`sort_only(path, col)`](SchemaBuilder::sort_only) adds a sortable column
    ///   that carries no filter declaration, e.g. `update_time`.
    ///
    /// ```ignore
    /// let schema = Schema::for_declarations(&site_declarations())
    ///     .column("lat_lng.latitude", "latitude")   // rename a flattened path
    ///     .sort_only("update_time", "update_time")   // sortable, not filterable
    ///     .build();
    /// ```
    pub fn for_declarations(declarations: &Declarations) -> SchemaBuilder {
        let mut builder = SchemaBuilder::default();
        for (path, ty) in declarations.field_paths() {
            builder.columns.insert(path.to_string(), path.to_string());
            builder.types.insert(path.to_string(), ty.clone());
            if is_sortable_type(ty) {
                builder.sortable.insert(path.to_string());
            }
        }
        builder
    }

    /// The SQL column a filter identifier / `order_by` path maps to, if the schema
    /// addresses it. Used by both [`transpile_filter`](crate::transpile_filter)
    /// and [`transpile_order_by`](crate::transpile_order_by); the sortable gate is
    /// [`sortable_paths`](Self::sortable_paths), applied upstream.
    pub(crate) fn column(&self, identifier: &str) -> Option<&str> {
        self.columns.get(identifier).map(String::as_str)
    }

    /// The declared [`Type`] of a SQL `column`, for columns derived from
    /// [`Declarations`]. The cursor page token decoder checks each cursor value's
    /// variant against this; a hand-built or key column returns `None` (key
    /// columns are uniformly text by AIP-122).
    pub fn column_type(&self, column: &str) -> Option<&Type> {
        self.types.get(column)
    }

    /// The field paths an `order_by` is allowed to sort on, sorted for a stable
    /// result — feed this to
    /// [`OrderBy::validate_for_paths`](aip_ordering::OrderBy::validate_for_paths)
    /// as the sort allowlist.
    ///
    /// These are the *filter paths* (the schema keys, e.g. `lat_lng.latitude`),
    /// not the column names: validation runs against the AIP field paths the user
    /// writes, and [`transpile_order_by`](crate::transpile_order_by) maps them to
    /// columns afterwards. Keeping this the single sortable source is the point of
    /// the crate — there is no third hand-spelled path list to drift.
    pub fn sortable_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.sortable.iter().map(String::as_str).collect();
        paths.sort_unstable();
        paths
    }
}

/// Whether a declared [`Type`] is one a SQL `ORDER BY` totally orders, and so
/// drives the [`for_declarations`](Schema::for_declarations) sortable rule. The
/// scalar orders — string, the numeric kinds, bool, timestamp — qualify; an
/// [`Enum`](Type::Enum) (no meaningful sort order over value names), a map, a
/// list, a [`Duration`](Type::Duration), or any other type does not.
fn is_sortable_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::String | Type::Int | Type::Uint | Type::Double | Type::Bool | Type::Timestamp
    )
}

/// Builder for a [`Schema`], mirroring `aip-filtering`'s declarations builder.
///
/// Start it empty with [`Schema::builder`] (every column filterable + sortable)
/// or pre-seeded from declarations with [`Schema::for_declarations`] (columns
/// derived, sortability type-driven), then layer overrides.
#[derive(Debug, Default)]
pub struct SchemaBuilder {
    columns: HashMap<String, String>,
    sortable: HashSet<String>,
    /// Declared type per filter path, seeded by [`Schema::for_declarations`];
    /// resolved to a column-keyed map in [`build`](Self::build).
    types: HashMap<String, Type>,
}

impl SchemaBuilder {
    /// Map filter `identifier` to SQL `column`, filterable **and** sortable. A
    /// repeated identifier replaces the earlier mapping.
    ///
    /// On a hand-built [`Schema::builder`] this is the way to add a column. On a
    /// [`Schema::for_declarations`] schema it **renames** an already-derived
    /// column whose SQL name differs from its declared path — `lat_lng.latitude`
    /// → `latitude` — leaving that path's derived sortability in place.
    pub fn column(mut self, identifier: &str, column: &str) -> Self {
        self.columns
            .insert(identifier.to_string(), column.to_string());
        self.sortable.insert(identifier.to_string());
        self
    }

    /// Add a **sort-only** column: sortable, with no filter declaration behind it.
    ///
    /// For columns a consumer pages on but does not filter — `update_time`, a
    /// flattened `lat_lng.longitude` — so an `order_by` may name them
    /// ([`sortable_paths`](Schema::sortable_paths) includes them) while no filter
    /// declares them, so the type-checker rejects them in a `filter`. It is also
    /// the escape hatch for making a type-driven filter-only column sortable
    /// anyway, e.g. `.sort_only("state", "state")`. A repeated identifier replaces
    /// the earlier mapping.
    pub fn sort_only(mut self, identifier: &str, column: &str) -> Self {
        self.columns
            .insert(identifier.to_string(), column.to_string());
        self.sortable.insert(identifier.to_string());
        self
    }

    /// Finalize the schema.
    pub fn build(self) -> Schema {
        // Re-key the per-path types onto their SQL columns, so a renamed path
        // (`lat_lng.latitude` → `latitude`) carries its type to the column.
        let mut types = HashMap::new();
        for (path, column) in &self.columns {
            if let Some(ty) = self.types.get(path) {
                types.insert(column.clone(), ty.clone());
            }
        }
        Schema {
            columns: self.columns,
            sortable: self.sortable,
            types,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aip_filtering::{Declarations, Type};

    /// Declarations with one field of each shape the sortable rule must classify.
    fn declarations() -> Declarations {
        Declarations::builder()
            .ident("display_name", Type::String)
            .ident("size", Type::Int)
            .ident("lat_lng.latitude", Type::Double)
            .ident("create_time", Type::Timestamp)
            .ident("ttl", Type::Duration)
            .ident("tags", Type::list(Type::String))
            .ident("annotations", Type::map(Type::String, Type::String))
            .ident("state", Type::Enum("example.State".to_owned()))
            .build()
    }

    #[test]
    fn derives_a_column_per_declared_field_defaulting_to_the_path() {
        let schema = Schema::for_declarations(&declarations()).build();
        // Every declared field becomes a column named for its path by default.
        assert_eq!(schema.column("display_name"), Some("display_name"));
        assert_eq!(schema.column("lat_lng.latitude"), Some("lat_lng.latitude"));
        assert_eq!(schema.column("state"), Some("state"));
    }

    #[test]
    fn sortable_is_type_driven_excluding_enum_map_list_and_duration() {
        let schema = Schema::for_declarations(&declarations()).build();
        // Scalars an ORDER BY totally orders are sortable...
        assert_eq!(
            schema.sortable_paths(),
            vec!["create_time", "display_name", "lat_lng.latitude", "size"],
        );
        // ...enum / map / list / duration are filter-only.
        for filter_only in ["state", "tags", "annotations", "ttl"] {
            assert!(
                !schema.sortable_paths().contains(&filter_only),
                "{filter_only} must not be sortable",
            );
        }
    }

    #[test]
    fn column_override_renames_without_dropping_sortability() {
        let schema = Schema::for_declarations(&declarations())
            .column("lat_lng.latitude", "latitude")
            .build();
        assert_eq!(schema.column("lat_lng.latitude"), Some("latitude"));
        assert!(schema.sortable_paths().contains(&"lat_lng.latitude"));
    }

    #[test]
    fn sort_only_adds_a_sortable_column_with_no_filter_declaration() {
        let schema = Schema::for_declarations(&declarations())
            .sort_only("update_time", "update_time")
            .build();
        assert_eq!(schema.column("update_time"), Some("update_time"));
        assert!(schema.sortable_paths().contains(&"update_time"));
    }

    #[test]
    fn sort_only_is_the_escape_hatch_for_a_filter_only_column() {
        let schema = Schema::for_declarations(&declarations())
            .sort_only("state", "state")
            .build();
        assert!(schema.sortable_paths().contains(&"state"));
    }

    #[test]
    fn hand_built_columns_default_to_filterable_and_sortable() {
        let schema = Schema::builder()
            .column("display_name", "display_name")
            .build();
        assert_eq!(schema.column("display_name"), Some("display_name"));
        assert_eq!(schema.sortable_paths(), vec!["display_name"]);
    }
}
