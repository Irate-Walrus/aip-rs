//! The [`Dialect`] trait, its single-pass renderer, and the SQLite dialect.

use crate::predicate::{Column, Predicate, Value};

/// Spells a [`Predicate`]'s leaves for one SQL engine and renders a whole
/// `Predicate` to `(sql, Vec<Value>)`.
///
/// The boolean walk — precedence parenthesization and left-to-right placeholder
/// numbering — is shared in the provided [`render`](Dialect::render); the one
/// per-engine knob in this slice is [`placeholder`](Dialect::placeholder)
/// (`?n` for SQLite, `$n` for Postgres). Ship SQLite first (ADR-0008); further
/// engines are one impl away.
pub trait Dialect {
    /// The placeholder text for the 1-based bind position `n`.
    fn placeholder(&self, n: usize) -> String;

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
}
