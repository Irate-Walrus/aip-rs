//! Walking the AIP-160 [`Filter`] AST into a [`Predicate`].

use aip_filtering::{function, Constant, Expr, Filter};

use crate::predicate::{Predicate, Value};
use crate::schema::Schema;
use crate::Error;

/// Transpile a type-checked [`Filter`] into a [`Predicate`], mapping each
/// identifier to its SQL column via `schema`.
///
/// This tracer-bullet slice handles only `=` and `AND` over scalar columns
/// (ADR-0008): every other construct is [`Error::Unsupported`]. Every literal
/// becomes a bound [`Value`] — nothing is spliced into SQL text.
pub fn transpile_filter(filter: &Filter, schema: &Schema) -> Result<Predicate, Error> {
    transpile_expr(&filter.expr, schema)
}

/// Transpile one boolean expression node.
fn transpile_expr(expr: &Expr, schema: &Schema) -> Result<Predicate, Error> {
    match expr {
        Expr::Call { function, args } => transpile_call(function, args, schema),
        other => Err(Error::Unsupported(format!(
            "expected a comparison or `AND`, found {other:?}"
        ))),
    }
}

/// Transpile a call node — `AND` lowers to [`Predicate::All`]; `=` to a
/// [`Predicate::Eq`] leaf.
fn transpile_call(name: &str, args: &[Expr], schema: &Schema) -> Result<Predicate, Error> {
    match (name, args) {
        (function::AND, [left, right]) => Ok(Predicate::all([
            transpile_expr(left, schema)?,
            transpile_expr(right, schema)?,
        ])),
        (function::EQUALS, [left, right]) => transpile_eq(left, right, schema),
        (function::AND | function::EQUALS, _) => Err(Error::Unsupported(format!(
            "`{name}` expects two operands, found {}",
            args.len()
        ))),
        _ => Err(Error::Unsupported(format!("operator `{name}`"))),
    }
}

/// Transpile an `=` whose operands are a column identifier and a literal, in
/// either order (`display_name = "x"` or `"x" = display_name`).
fn transpile_eq(left: &Expr, right: &Expr, schema: &Schema) -> Result<Predicate, Error> {
    let (ident, literal) = match (left, right) {
        (Expr::Ident(ident), Expr::Const(literal)) | (Expr::Const(literal), Expr::Ident(ident)) => {
            (ident, literal)
        }
        _ => {
            return Err(Error::Unsupported(
                "`=` must compare a scalar column to a literal".to_string(),
            ))
        }
    };
    let column = schema
        .column(ident)
        .ok_or_else(|| Error::UnknownIdentifier(ident.clone()))?;
    Ok(Predicate::eq(column, to_value(literal)))
}

/// Lift a filter [`Constant`] into a bound [`Value`].
fn to_value(constant: &Constant) -> Value {
    match constant {
        Constant::Int(i) => Value::Int(*i),
        // SQLite has no unsigned type; widen into a signed bind (lossy only above
        // i64::MAX, which no scalar id column reaches in this demo).
        Constant::Uint(u) => Value::Int(*u as i64),
        Constant::Double(d) => Value::Double(*d),
        Constant::Bool(b) => Value::Bool(*b),
        Constant::String(s) => Value::Text(s.clone()),
        Constant::Bytes(b) => Value::Bytes(b.clone()),
    }
}
