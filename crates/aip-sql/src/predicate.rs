//! The composable boolean [`Predicate`], its comparison leaves, and bound
//! [`Value`]s.

/// A bind value — an executor-agnostic literal pulled out of a filter so it is
/// bound as a parameter, never spliced into SQL text (ADR-0005 / ADR-0008). The
/// caller maps it onto its driver's parameter type.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A boolean (bound as the driver sees fit — `0`/`1` for SQLite).
    Bool(bool),
    /// A signed 64-bit integer. AIP-160 `uint` literals are widened into this.
    Int(i64),
    /// A 64-bit float. A `duration(...)` literal is bound here as its total
    /// seconds (ADR-0008 amendment #40).
    Double(f64),
    /// A UTF-8 string. A `timestamp(...)` / RFC3339 literal is bound here as its
    /// text, and an enum value as its value name (ADR-0008 amendment #40).
    Text(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

/// A comparison operator between a column (or map member) and a bound value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// Equality (`=`).
    Eq,
    /// Inequality (`!=`).
    Ne,
    /// Less-than (`<`).
    Lt,
    /// Less-than-or-equal (`<=`).
    Le,
    /// Greater-than (`>`).
    Gt,
    /// Greater-than-or-equal (`>=`).
    Ge,
}

impl CmpOp {
    /// The SQL spelling of this operator. The comparison operators are standard
    /// SQL — identical across SQLite and Postgres — so the renderer writes them
    /// directly rather than through the [`Dialect`](crate::Dialect).
    pub(crate) fn sql(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }

    /// This operator with its operands swapped, so a `value <op> column`
    /// restriction can be rewritten into the canonical `column <op> value` form
    /// the renderer emits. `=` / `!=` are symmetric; the ordering operators flip.
    pub(crate) fn mirror(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Eq,
            CmpOp::Ne => CmpOp::Ne,
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
        }
    }
}

/// The left side of a comparison: a plain column, or a value selected from a
/// stored JSON `map` column by key (the AIP-160 member access `labels.env`).
#[derive(Debug, Clone, PartialEq)]
pub enum Column {
    /// A bare SQL column.
    Plain(String),
    /// A value read from a `map`-typed column by key, rendered `column ->> ?`
    /// with the key bound — it comes from the filter, so it is never spliced into
    /// the SQL text (ADR-0005 / ADR-0008). The `->>` JSON accessor is shared by
    /// SQLite and Postgres.
    MapMember {
        /// The SQL column holding the JSON map.
        column: String,
        /// The map key to read — a filter-supplied value, bound at render time.
        key: String,
    },
}

/// The presence/membership test of an AIP-160 has operator (`:`) leaf — the
/// per-engine-spelled part of a [`Predicate::Has`].
///
/// Each variant is one overload the checker accepts: a substring of a string, a
/// key in a `map<string,string>`, a value in a `list<string>`, or presence of a
/// timestamp (`field:*`). The value it carries (where one applies) comes from the
/// filter, so it is bound at render time, never spliced into SQL text (ADR-0005 /
/// ADR-0008). How each is spelled is the [`Dialect`](crate::Dialect)'s job — the
/// substring `LIKE` and the `json_each` membership tests are the main per-engine
/// divergence (ADR-0008).
#[derive(Debug, Clone, PartialEq)]
pub enum HasTest {
    /// Substring match on a string column (`field:value`): a `LIKE` whose bound
    /// pattern wraps the value in `%…%` with its `LIKE` metacharacters escaped,
    /// so user input can never act as a wildcard.
    Substring(String),
    /// Key presence in a `map<string,string>` column (`map:key`).
    Key(String),
    /// Value presence in a `list<string>` column (`list:value`).
    Element(String),
    /// Presence of a value (timestamp `field:*`). The checker restricts the has
    /// operator on a timestamp to the `*` wildcard, so this binds nothing.
    Present,
}

/// A composable, parameterized boolean SQL fragment.
///
/// Its logical structure (`AND` / `OR` / `NOT`) is portable; its leaves are
/// *spelled* by a [`Dialect`](crate::Dialect), which numbers the placeholders.
/// Build one with [`transpile_filter`](crate::transpile_filter) or the
/// [`all`](Predicate::all) / [`any`](Predicate::any) / [`not`](Predicate::not) /
/// [`eq`](Predicate::eq) / [`is_null`](Predicate::is_null) /
/// [`scope_to_parent`](Predicate::scope_to_parent) / [`raw`](Predicate::raw)
/// constructors, then render it with [`Dialect::render`](crate::Dialect::render).
///
/// Centralizing precedence and placeholder numbering here is the whole point: a
/// server can compose a user's filter with its own predicates — parent scoping,
/// tenancy, soft delete — without the bare-string footguns (`a OR b` silently
/// re-binding as `a OR (b AND …)`, or two independently-numbered `?1` parameters
/// colliding). See ADR-0008.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// Conjunction (`a AND b AND …`). An empty `All` is always true.
    All(Vec<Predicate>),
    /// Disjunction (`a OR b OR …`). An empty `Any` is always false.
    Any(Vec<Predicate>),
    /// Negation (`NOT a`).
    Not(Box<Predicate>),
    /// A comparison between a column (or map member) and a bound value
    /// (`col <op> ?`).
    Compare {
        /// The left-hand column — a plain column or a map-member access.
        column: Column,
        /// The comparison operator.
        op: CmpOp,
        /// The bound right-hand value.
        value: Value,
    },
    /// An AIP-160 has operator (`:`) presence/membership test on a column —
    /// substring, map-key, list-element, or presence. The per-engine leaf, spelled
    /// by [`Dialect::render_has`](crate::Dialect::render_has).
    Has {
        /// The SQL column under test.
        column: String,
        /// Which presence/membership test to apply, plus its bound value.
        test: HasTest,
    },
    /// A `column IS NULL` test, e.g. the soft-delete predicate `delete_time IS
    /// NULL` a server composes alongside a user filter.
    IsNull(String),
    /// An AIP parent scope (`column LIKE ?n ESCAPE '\'`): a resource-name prefix
    /// match keeping the rows under a parent. The bound pattern escapes the
    /// parent's `LIKE` metacharacters (`%` / `_` / `\`) and appends the child
    /// wildcard, so a parent containing `%` or `_` matches literally and is never
    /// interpolated. Built with [`scope_to_parent`](Predicate::scope_to_parent).
    Scope {
        /// The resource-name column to scope (e.g. `name`).
        column: String,
        /// The parent resource name whose children to keep — escaped and bound at
        /// render time, never spliced into SQL text.
        parent: String,
    },
    /// A verbatim boolean SQL fragment — the escape hatch for a server predicate
    /// the typed builders don't cover. It carries no bind values (so it never
    /// perturbs the shared placeholder numbering) and is rendered as-is; because
    /// its internal structure is opaque, it is always parenthesized when composed
    /// under a combinator. Build with [`raw`](Predicate::raw).
    Raw(String),
}

impl Predicate {
    /// A conjunction of `parts` (`a AND b AND …`).
    pub fn all(parts: impl IntoIterator<Item = Predicate>) -> Self {
        Predicate::All(parts.into_iter().collect())
    }

    /// A disjunction of `parts` (`a OR b OR …`).
    pub fn any(parts: impl IntoIterator<Item = Predicate>) -> Self {
        Predicate::Any(parts.into_iter().collect())
    }

    /// The negation of `inner` (`NOT inner`).
    // `not` is the builder name ADR-0008 fixes; it is a constructor, not the
    // `std::ops::Not` operator clippy assumes.
    #[allow(clippy::should_implement_trait)]
    pub fn not(inner: Predicate) -> Self {
        Predicate::Not(Box::new(inner))
    }

    /// A column-equals-value comparison (`column = ?`), binding `value`.
    pub fn eq(column: impl Into<String>, value: Value) -> Self {
        Predicate::Compare {
            column: Column::Plain(column.into()),
            op: CmpOp::Eq,
            value,
        }
    }

    /// A `column IS NULL` test — e.g. the soft-delete predicate
    /// `is_null("delete_time")`.
    pub fn is_null(column: impl Into<String>) -> Self {
        Predicate::IsNull(column.into())
    }

    /// An AIP parent scope on a resource-name `column`: a `LIKE` prefix keeping
    /// the rows whose name lies under `parent`. The parent's `LIKE`
    /// metacharacters are escaped and the whole pattern is bound, so a parent
    /// containing `%` or `_` matches literally and is never interpolated
    /// (ADR-0008).
    pub fn scope_to_parent(column: impl Into<String>, parent: impl Into<String>) -> Self {
        Predicate::Scope {
            column: column.into(),
            parent: parent.into(),
        }
    }

    /// A verbatim boolean SQL fragment — the escape hatch for a server predicate
    /// the typed builders don't cover. `sql` must carry no bind placeholders (it
    /// does not participate in the shared numbering); anything needing a bound
    /// value belongs in [`eq`](Predicate::eq) / [`is_null`](Predicate::is_null) /
    /// [`scope_to_parent`](Predicate::scope_to_parent) or a composition of them.
    pub fn raw(sql: impl Into<String>) -> Self {
        Predicate::Raw(sql.into())
    }

    /// Binding tightness, used by the renderer to parenthesize a child only when
    /// it binds looser than its parent. Higher binds tighter, mirroring SQL:
    /// a leaf (comparison, has test, `IS NULL`, scope) > `NOT` > `AND` > `OR` >
    /// a raw fragment.
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            // A has leaf renders as a self-contained atom (`LIKE …`, `EXISTS
            // (…)`, `… IS NOT NULL`), so it binds as tight as a comparison; an
            // `IS NULL` test and a `LIKE`-prefix scope are atoms too.
            Predicate::Compare { .. }
            | Predicate::Has { .. }
            | Predicate::IsNull(_)
            | Predicate::Scope { .. } => 4,
            Predicate::Not(_) => 3,
            Predicate::All(_) => 2,
            Predicate::Any(_) => 1,
            // A raw fragment's internal structure is opaque, so it is treated as
            // the loosest-binding node: it is always parenthesized when composed
            // under any combinator, never silently re-associating.
            Predicate::Raw(_) => 0,
        }
    }
}
