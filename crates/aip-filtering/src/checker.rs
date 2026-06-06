//! Type-checker for the AIP-160 comparison slice.
//!
//! Resolves every identifier against the [`Declarations`] allowlist, assigns a
//! [`Type`] to each node, resolves the comparison operator's overload, and
//! requires the whole filter to evaluate to a `bool`. Reflection-free: a member
//! path (`book.author`) is matched against the declared identifier names, so
//! map/message field typing is left to a later slice.

use crate::{Constant, Declarations, Error, Expr, Type};

/// Type-check `expr` against `declarations`, requiring a boolean result.
pub(crate) fn check(expr: &Expr, declarations: &Declarations) -> Result<(), Error> {
    let result = type_of(expr, declarations)?;
    if result != Type::Bool {
        return Err(Error::Type(format!(
            "filter must evaluate to a bool, but has type {result:?}"
        )));
    }
    Ok(())
}

/// The [`Type`] of an expression, or a type/undeclared-identifier error.
fn type_of(expr: &Expr, declarations: &Declarations) -> Result<Type, Error> {
    match expr {
        Expr::Const(constant) => Ok(constant_type(constant)),
        Expr::Ident(name) => lookup(name, declarations),
        Expr::Select { .. } => {
            let name = qualified_name(expr)
                .ok_or_else(|| Error::Type("unsupported field selection".to_string()))?;
            lookup(&name, declarations)
        }
        Expr::Call { function, args } => check_call(function, args, declarations),
    }
}

/// Type-check a comparison call: both operands are checked, then the operator's
/// overload is resolved over their types.
fn check_call(function: &str, args: &[Expr], declarations: &Declarations) -> Result<Type, Error> {
    let [lhs, rhs] = args else {
        return Err(Error::Type(format!(
            "comparison `{function}` expects 2 arguments, got {}",
            args.len()
        )));
    };
    let left = type_of(lhs, declarations)?;
    let right = type_of(rhs, declarations)?;
    resolve_comparison(function, &left, &right)
}

/// Resolve a comparison operator over its operand types, yielding `bool` or a
/// type error. The overloads mirror `aip-go`'s standard primitives; timestamp,
/// duration, and enum overloads arrive with their later slices.
fn resolve_comparison(function: &str, left: &Type, right: &Type) -> Result<Type, Error> {
    use Type::{Bool, Double, Int, String};
    let matched = match function {
        "=" | "!=" => matches!(
            (left, right),
            (Bool, Bool) | (Int, Int) | (Double, Double) | (Double, Int) | (String, String)
        ),
        "<" | "<=" | ">" | ">=" => matches!(
            (left, right),
            (Int, Int) | (Double, Double) | (Double, Int) | (String, String)
        ),
        _ => return Err(Error::Type(format!("undeclared function `{function}`"))),
    };
    if matched {
        Ok(Bool)
    } else {
        Err(Error::Type(format!(
            "no overload for `{function}` with ({left:?}, {right:?})"
        )))
    }
}

/// Look an identifier up in the allowlist.
fn lookup(name: &str, declarations: &Declarations) -> Result<Type, Error> {
    declarations
        .lookup_ident(name)
        .cloned()
        .ok_or_else(|| Error::UndeclaredIdent(name.to_string()))
}

/// The CEL-equivalent type of a literal constant.
fn constant_type(constant: &Constant) -> Type {
    match constant {
        Constant::Int(_) => Type::Int,
        Constant::Uint(_) => Type::Uint,
        Constant::Double(_) => Type::Double,
        Constant::Bool(_) => Type::Bool,
        Constant::String(_) => Type::String,
        Constant::Bytes(_) => Type::Bytes,
    }
}

/// The dotted identifier name of a member chain (`book.author`), or `None` if the
/// chain is not a plain identifier/selection.
fn qualified_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name) => Some(name.clone()),
        Expr::Select { operand, field } => Some(format!("{}.{field}", qualified_name(operand)?)),
        _ => None,
    }
}
