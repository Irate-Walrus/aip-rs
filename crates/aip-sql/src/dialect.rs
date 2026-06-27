//! The [`Dialect`] trait, its single-pass renderer, and the SQLite dialect.

use crate::predicate::{Column, HasTest, Predicate, Value};

/// Spells a [`Predicate`]'s leaves for one SQL engine and renders a whole
/// `Predicate` to `(sql, Vec<Value>)`.
///
/// The boolean walk — precedence parenthesization and left-to-right placeholder
/// numbering — is shared in the provided [`render`](Dialect::render); the
/// per-engine knobs are [`placeholder`](Dialect::placeholder) (`?n` for SQLite,
/// `$n` for Postgres) and [`render_has`](Dialect::render_has), the has operator
/// `:` leaf (substring `LIKE`, map-key / list-element membership) — the main
/// divergence between engines (ADR-0008). Ship SQLite first; further engines are
/// one impl away.
pub trait Dialect {
    /// The placeholder text for the 1-based bind position `n`.
    fn placeholder(&self, n: usize) -> String;

    /// Spell an AIP-160 has operator (`:`) leaf on `column`: a substring `LIKE`,
    /// a map-key / list-element membership test, or a presence test. Returns the
    /// leaf's SQL plus its ordered bind values; `next_bind` is the 1-based
    /// placeholder number for its first bound value, so the leaf numbers in step
    /// with the surrounding [`render`](Dialect::render) pass. The bound value (a
    /// substring, key, or element) is filter input, never spliced into the SQL
    /// text (ADR-0005 / ADR-0008). This is the main per-engine divergence.
    fn render_has(&self, column: &str, test: &HasTest, next_bind: usize) -> (String, Vec<Value>);

    /// Render `predicate` to a complete SQL boolean expression plus its ordered
    /// bind values, assigning every placeholder in one left-to-right pass and
    /// parenthesizing by precedence. The returned SQL has no enclosing parens.
    fn render(&self, predicate: &Predicate) -> (String, Vec<Value>) {
        let mut sql = String::new();
        let mut binds = Vec::new();
        write_node(self, predicate, &mut sql, &mut binds);
        (sql, binds)
    }
}

/// Append `node`'s SQL (without enclosing parens) to `sql`, pushing its bind
/// values to `binds` in render order so each placeholder number matches its slot.
fn write_node<D: Dialect + ?Sized>(
    dialect: &D,
    node: &Predicate,
    sql: &mut String,
    binds: &mut Vec<Value>,
) {
    match node {
        Predicate::Compare { column, op, value } => {
            // Render the left side first (a map member emits its own bound key),
            // then ` <op> `, then the right-hand value placeholder. One pass, so
            // each placeholder number is the count of binds preceding it.
            write_column(dialect, column, sql, binds);
            sql.push(' ');
            sql.push_str(op.sql());
            sql.push(' ');
            sql.push_str(&dialect.placeholder(binds.len() + 1));
            binds.push(value.clone());
        }
        // The has operator leaf is spelled per-engine; it numbers from the next
        // free bind slot so its placeholder stays in left-to-right order.
        Predicate::Has { column, test } => {
            let (leaf_sql, leaf_binds) = dialect.render_has(column, test, binds.len() + 1);
            sql.push_str(&leaf_sql);
            binds.extend(leaf_binds);
        }
        // `IS NULL` is standard SQL across engines, so it is rendered directly
        // (like the comparison operators) and binds nothing.
        Predicate::IsNull(column) => {
            sql.push_str(column);
            sql.push_str(" IS NULL");
        }
        // A keyset cursor seek: the row-value comparison `(a, b) > (?1, ?2)`,
        // standard SQL across SQLite and Postgres. Each value binds in column
        // order, so the placeholders stay left-to-right.
        Predicate::TupleGt { columns, values } => {
            sql.push('(');
            sql.push_str(&columns.join(", "));
            sql.push_str(") > (");
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    sql.push_str(", ");
                }
                sql.push_str(&dialect.placeholder(binds.len() + 1));
                binds.push(value.clone());
            }
            sql.push(')');
        }
        // A raw fragment is emitted verbatim; it carries no binds, so the shared
        // placeholder numbering is untouched. `write_child` parenthesizes it when
        // it sits under a combinator (its precedence is the loosest).
        Predicate::Raw(fragment) => sql.push_str(fragment),
        Predicate::Not(inner) => {
            sql.push_str("NOT ");
            write_child(dialect, inner, node.precedence(), sql, binds);
        }
        // An empty conjunction is vacuously true; an empty disjunction false.
        Predicate::All(parts) => write_join(
            dialect,
            parts,
            " AND ",
            node.precedence(),
            "1 = 1",
            sql,
            binds,
        ),
        Predicate::Any(parts) => write_join(
            dialect,
            parts,
            " OR ",
            node.precedence(),
            "1 = 0",
            sql,
            binds,
        ),
    }
}

/// Append the left side of a comparison: a plain column verbatim, or a map
/// member as `column ->> ?` with its key bound. The key bind is emitted here, so
/// it precedes the comparison's right-hand value in the single left-to-right
/// pass and its placeholder number lands one before the value's.
fn write_column<D: Dialect + ?Sized>(
    dialect: &D,
    column: &Column,
    sql: &mut String,
    binds: &mut Vec<Value>,
) {
    match column {
        Column::Plain(name) => sql.push_str(name),
        Column::MapMember { column, key } => {
            sql.push_str(column);
            sql.push_str(" ->> ");
            sql.push_str(&dialect.placeholder(binds.len() + 1));
            binds.push(Value::Text(key.clone()));
        }
    }
}

/// Append `child`, wrapping it in parens only when it binds looser than its
/// parent (`parent_precedence`) — the minimal-but-correct parenthesization.
fn write_child<D: Dialect + ?Sized>(
    dialect: &D,
    child: &Predicate,
    parent_precedence: u8,
    sql: &mut String,
    binds: &mut Vec<Value>,
) {
    if child.precedence() < parent_precedence {
        sql.push('(');
        write_node(dialect, child, sql, binds);
        sql.push(')');
    } else {
        write_node(dialect, child, sql, binds);
    }
}

/// Append `parts` joined by `separator`, each parenthesized as needed; an empty
/// list renders `empty`.
fn write_join<D: Dialect + ?Sized>(
    dialect: &D,
    parts: &[Predicate],
    separator: &str,
    precedence: u8,
    empty: &str,
    sql: &mut String,
    binds: &mut Vec<Value>,
) {
    if parts.is_empty() {
        sql.push_str(empty);
        return;
    }
    for (i, child) in parts.iter().enumerate() {
        if i > 0 {
            sql.push_str(separator);
        }
        write_child(dialect, child, precedence, sql, binds);
    }
}

/// The SQLite [`Dialect`]: numbered `?n` placeholders.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sqlite;

impl Dialect for Sqlite {
    fn placeholder(&self, n: usize) -> String {
        format!("?{n}")
    }

    fn render_has(&self, column: &str, test: &HasTest, next_bind: usize) -> (String, Vec<Value>) {
        let placeholder = self.placeholder(next_bind);
        match test {
            // Substring: a `LIKE` whose bound pattern wraps the value in `%…%`
            // with its `LIKE` metacharacters escaped under an explicit `ESCAPE`,
            // so user input matches literally and never as a wildcard.
            HasTest::Substring(value) => (
                format!("{column} LIKE {placeholder} ESCAPE '\\'"),
                vec![Value::Text(format!("%{}%", escape_like(value)))],
            ),
            // Map-key presence: the key exists iff `json_each` over the stored map
            // yields a row whose `key` equals the bound key.
            HasTest::Key(key) => (
                format!("EXISTS (SELECT 1 FROM json_each({column}) WHERE key = {placeholder})"),
                vec![Value::Text(key.clone())],
            ),
            // List-element membership: the value is present iff `json_each` over
            // the stored list yields a row whose `value` equals the bound value.
            HasTest::Element(value) => (
                format!("EXISTS (SELECT 1 FROM json_each({column}) WHERE value = {placeholder})"),
                vec![Value::Text(value.clone())],
            ),
            // Presence (`field:*`) is a `NULL` test, so it binds nothing.
            HasTest::Present => (format!("{column} IS NOT NULL"), Vec::new()),
        }
    }
}

/// Escape a value's `LIKE` metacharacters — the wildcards `%` and `_`, and the
/// escape character `\` itself — so a substring match treats them literally.
/// Pairs with a `LIKE … ESCAPE '\'` clause; the backslash is escaped first so it
/// does not double-escape the `%` / `_` that follow.
fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}
