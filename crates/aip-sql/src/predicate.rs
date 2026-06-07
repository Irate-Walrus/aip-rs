//! The composable boolean [`Predicate`] and its bound [`Value`]s.

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
    /// A 64-bit float.
    Double(f64),
    /// A UTF-8 string.
    Text(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

/// A composable, parameterized boolean SQL fragment.
///
/// Its logical structure (`AND` / `OR` / `NOT`) is portable; its leaves are
/// *spelled* by a [`Dialect`](crate::Dialect), which also numbers the
/// placeholders. Build one with [`transpile_filter`](crate::transpile_filter) or
/// the [`all`](Predicate::all) / [`any`](Predicate::any) / [`not`](Predicate::not)
/// / [`eq`](Predicate::eq) constructors, then render it with
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
    /// An equality between a column and a bound value (`col = ?`).
    Eq {
        /// The SQL column name (already mapped from a filter identifier).
        column: String,
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
        Predicate::Eq {
            column: column.into(),
            value,
        }
    }

    /// Binding tightness, used by the renderer to parenthesize a child only when
    /// it binds looser than its parent. Higher binds tighter, mirroring SQL:
    /// comparison > `NOT` > `AND` > `OR`.
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            Predicate::Eq { .. } => 4,
            Predicate::Not(_) => 3,
            Predicate::All(_) => 2,
            Predicate::Any(_) => 1,
        }
    }
}
