//! Type-checker for the AIP-160 filter grammar.
//!
//! Assigns a [`Type`] to each node: identifiers (and dotted member chains)
//! resolve against the [`Declarations`] allowlist, calls resolve a declared
//! function overload over their argument types, and the whole filter must
//! evaluate to a `bool`. Reflection-free: a member path (`book.author`) is
//! matched against the declared identifier names, with map-valued operands the
//! one structural fallback (`com.google` where `com` is a `map`).

use crate::{function, Constant, Declarations, Error, Expr, Type};

/// Type-check `expr` against `declarations`, requiring a boolean result.
pub(crate) fn check(expr: &Expr, declarations: &Declarations) -> Result<(), Error> {
    let result = type_of(expr, declarations)?;
    if result != Type::Bool {
        return Err(Error::Type("non-bool result type".to_string()));
    }
    Ok(())
}

/// The [`Type`] of an expression, or a type/undeclared-identifier error.
fn type_of(expr: &Expr, declarations: &Declarations) -> Result<Type, Error> {
    match expr {
        Expr::Const(constant) => Ok(constant_type(constant)),
        Expr::Ident(name) => lookup(name, declarations),
        Expr::Select { operand, field } => {
            // A fully-qualified member chain may be declared as one identifier.
            if let Some(name) = qualified_name(expr) {
                if let Some(ty) = declarations.lookup_ident(&name) {
                    return Ok(ty.clone());
                }
            }
            // Otherwise the operand must be a map, and selection yields its value.
            match type_of(operand, declarations)? {
                Type::Map(_, value) => Ok(*value),
                other => Err(Error::Type(format!(
                    "cannot select `{field}` on non-map type {other:?}"
                ))),
            }
        }
        Expr::Call { function, args } => check_call(function, args, declarations),
    }
}

/// Type-check a call: every argument is typed, then the function's overload set
/// is searched for one whose parameters match the argument types exactly.
fn check_call(function: &str, args: &[Expr], declarations: &Declarations) -> Result<Type, Error> {
    let arg_types = args
        .iter()
        .map(|arg| type_of(arg, declarations))
        .collect::<Result<Vec<_>, _>>()?;
    let overloads = declarations
        .lookup_function(function)
        .ok_or_else(|| Error::Type(format!("undeclared function '{function}'")))?;
    for overload in overloads {
        if overload.params == arg_types {
            // The has operator on a timestamp is presence-only: `field:*` checks
            // presence, but comparing the field to a concrete value is rejected.
            if function == function::HAS && arg_types == [Type::Timestamp, Type::String] {
                if let Expr::Const(Constant::String(value)) = &args[1] {
                    if value != "*" {
                        return Err(Error::Type(
                            "the has operator on timestamp fields only supports \
                             the wildcard \"*\" for presence checks"
                                .to_string(),
                        ));
                    }
                }
            }
            return Ok(overload.result.clone());
        }
    }
    Err(Error::Type(format!(
        "no matching overload found for calling '{function}' with {arg_types:?}"
    )))
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
