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

/// A composable, parameterized boolean SQL fragment.
///
/// Its logical structure (`AND` / `OR` / `NOT`) is portable; its leaves are
/// *spelled* by a [`Dialect`](crate::Dialect), which numbers the placeholders.
/// Build one with [`transpile_filter`](crate::transpile_filter) or the
/// [`all`](Predicate::all) / [`any`](Predicate::any) / [`not`](Predicate::not) /
/// [`eq`](Predicate::eq) constructors, then render it with
/// [`Dialect::render`](crate::Dialect::render).
///
/// Centralizing precedence and placeholder numbering here is the whole point: a
/// server can compose a user's filter with its own predicates without the
/// bare-string footguns (`a OR b` silently re-binding as `a OR (b AND …)`, or two
/// independently-numbered `?1` parameters colliding). See ADR-0008.
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

    /// Binding tightness, used by the renderer to parenthesize a child only when
    /// it binds looser than its parent. Higher binds tighter, mirroring SQL:
    /// comparison > `NOT` > `AND` > `OR`.
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            Predicate::Compare { .. } => 4,
            Predicate::Not(_) => 3,
            Predicate::All(_) => 2,
            Predicate::Any(_) => 1,
        }
    }
}
