//! Conversion between the native AST and `google.api.expr.v1alpha1` CEL protos.
//!
//! The native [`Expr`](crate::Expr) enum is filtering's primary product (see
//! `docs/adr/0003-native-filter-ast.md`); this module is the optional bridge to
//! CEL tooling that speaks the proto. The mapping is:
//!
//! | native                     | CEL proto                            |
//! |----------------------------|--------------------------------------|
//! | [`Expr`](crate::Expr)      | [`cel::Expr`]                        |
//! | [`Constant`](crate::Constant) | [`cel::Constant`]                 |
//! | [`Filter`](crate::Filter)  | [`cel::CheckedExpr`]                 |
//!
//! Native → proto is total ([`From`]); each [`cel::Expr`] node is assigned a
//! fresh, unique `id` (CEL expects non-zero ids), counting from `1` per tree.
//! Proto → native is partial ([`TryFrom`]): CEL is a superset, so a node the
//! native AST does not model (list/struct/comprehension literals, method-style
//! calls, presence-test selects, the `null`/`duration`/`timestamp` constants)
//! is rejected with a [`ConversionError`] rather than silently reshaped.
//!
//! A native [`Filter`](crate::Filter) holds only its expression tree, not the
//! per-node CEL types a real type-check produces, so [`cel::CheckedExpr`]'s
//! `type_map` / `reference_map` are left empty — the conversion preserves the
//! expression, not inferred type annotations.

use crate::{Constant, Expr, Filter};

/// The generated CEL `google.api.expr.v1alpha1` types (`Expr`, `Constant`,
/// `CheckedExpr`, ...). `build.rs` emits this module into `OUT_DIR` only when the
/// `cel-proto` feature is on; `google.protobuf.*` well-known types map to
/// [`prost_types`]. Generated code, so linting is relaxed here.
pub mod cel {
    #![allow(clippy::all, missing_docs, rustdoc::all)]
    include!(concat!(env!("OUT_DIR"), "/google.api.expr.v1alpha1.rs"));
}

/// A CEL proto could not be represented in the native AST.
///
/// Produced only by the proto → native ([`TryFrom`]) direction: CEL models more
/// than the AIP-160 subset the native AST covers, so these node kinds have no
/// native equivalent. Native → proto never fails.
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    /// A CEL expression kind the native AST does not model (e.g. a list, struct,
    /// or comprehension literal, or a method-style call with a target).
    #[error("unsupported CEL expression kind: {0}")]
    UnsupportedExpr(&'static str),
    /// A CEL constant kind the native AST does not model (`null`, or the
    /// deprecated `duration` / `timestamp` constants).
    #[error("unsupported CEL constant kind: {0}")]
    UnsupportedConstant(&'static str),
    /// A required field of a CEL message was absent (an unset oneof, or a
    /// missing operand / expression).
    #[error("malformed CEL proto: missing {0}")]
    Missing(&'static str),
}

// ---------------------------------------------------------------------------
// native -> CEL proto (total)
// ---------------------------------------------------------------------------

impl From<&Constant> for cel::Constant {
    fn from(constant: &Constant) -> Self {
        use cel::constant::ConstantKind;
        let kind = match constant {
            Constant::Int(v) => ConstantKind::Int64Value(*v),
            Constant::Uint(v) => ConstantKind::Uint64Value(*v),
            Constant::Double(v) => ConstantKind::DoubleValue(*v),
            Constant::Bool(v) => ConstantKind::BoolValue(*v),
            Constant::String(v) => ConstantKind::StringValue(v.clone()),
            Constant::Bytes(v) => ConstantKind::BytesValue(v.clone()),
        };
        cel::Constant {
            constant_kind: Some(kind),
        }
    }
}

impl From<Constant> for cel::Constant {
    fn from(constant: Constant) -> Self {
        (&constant).into()
    }
}

impl From<&Expr> for cel::Expr {
    fn from(expr: &Expr) -> Self {
        build_cel_expr(expr, &mut 1)
    }
}

impl From<Expr> for cel::Expr {
    fn from(expr: Expr) -> Self {
        (&expr).into()
    }
}

/// Recursively lowers a native [`Expr`] to a [`cel::Expr`], stamping each node
/// with the next id from `next_id` (pre-order, so a parent precedes its
/// children). Ids only need to be unique within the tree.
fn build_cel_expr(expr: &Expr, next_id: &mut i64) -> cel::Expr {
    use cel::expr::{Call, ExprKind, Ident, Select};

    let id = *next_id;
    *next_id += 1;

    let expr_kind = match expr {
        Expr::Const(constant) => ExprKind::ConstExpr(constant.into()),
        Expr::Ident(name) => ExprKind::IdentExpr(Ident { name: name.clone() }),
        Expr::Select { operand, field } => ExprKind::SelectExpr(Box::new(Select {
            operand: Some(Box::new(build_cel_expr(operand, next_id))),
            field: field.clone(),
            test_only: false,
        })),
        Expr::Call { function, args } => ExprKind::CallExpr(Box::new(Call {
            target: None,
            function: function.clone(),
            args: args
                .iter()
                .map(|arg| build_cel_expr(arg, next_id))
                .collect(),
        })),
    };

    cel::Expr {
        id,
        expr_kind: Some(expr_kind),
    }
}

impl From<&Filter> for cel::CheckedExpr {
    /// Wraps the filter's expression tree in a [`cel::CheckedExpr`]. The
    /// `type_map` / `reference_map` are empty: the native filter does not retain
    /// the per-node CEL types from checking (see the module docs).
    fn from(filter: &Filter) -> Self {
        cel::CheckedExpr {
            expr: Some((&filter.expr).into()),
            ..Default::default()
        }
    }
}

impl From<Filter> for cel::CheckedExpr {
    fn from(filter: Filter) -> Self {
        (&filter).into()
    }
}

// ---------------------------------------------------------------------------
// CEL proto -> native (partial)
// ---------------------------------------------------------------------------

impl TryFrom<cel::Constant> for Constant {
    type Error = ConversionError;

    // CEL's `duration` / `timestamp` constants are deprecated; we name those
    // variants only to reject them, so opt out of the deprecation lint here.
    #[allow(deprecated)]
    fn try_from(constant: cel::Constant) -> Result<Self, Self::Error> {
        use cel::constant::ConstantKind;
        match constant.constant_kind {
            Some(ConstantKind::BoolValue(v)) => Ok(Constant::Bool(v)),
            Some(ConstantKind::Int64Value(v)) => Ok(Constant::Int(v)),
            Some(ConstantKind::Uint64Value(v)) => Ok(Constant::Uint(v)),
            Some(ConstantKind::DoubleValue(v)) => Ok(Constant::Double(v)),
            Some(ConstantKind::StringValue(v)) => Ok(Constant::String(v)),
            Some(ConstantKind::BytesValue(v)) => Ok(Constant::Bytes(v)),
            Some(ConstantKind::NullValue(_)) => Err(ConversionError::UnsupportedConstant("null")),
            Some(ConstantKind::DurationValue(_)) => {
                Err(ConversionError::UnsupportedConstant("duration"))
            }
            Some(ConstantKind::TimestampValue(_)) => {
                Err(ConversionError::UnsupportedConstant("timestamp"))
            }
            None => Err(ConversionError::Missing("constant kind")),
        }
    }
}

impl TryFrom<cel::Expr> for Expr {
    type Error = ConversionError;

    fn try_from(expr: cel::Expr) -> Result<Self, Self::Error> {
        use cel::expr::ExprKind;
        match expr.expr_kind {
            Some(ExprKind::ConstExpr(constant)) => Ok(Expr::Const(constant.try_into()?)),
            Some(ExprKind::IdentExpr(ident)) => Ok(Expr::Ident(ident.name)),
            Some(ExprKind::SelectExpr(select)) => {
                let select = *select;
                // A presence-test select (`has(x.y)`) has no native equivalent;
                // reshaping it into a plain field access would change its meaning.
                if select.test_only {
                    return Err(ConversionError::UnsupportedExpr("presence-test select"));
                }
                let operand = select
                    .operand
                    .ok_or(ConversionError::Missing("select operand"))?;
                Ok(Expr::Select {
                    operand: Box::new((*operand).try_into()?),
                    field: select.field,
                })
            }
            Some(ExprKind::CallExpr(call)) => {
                let call = *call;
                // Native calls are function-style (`f(a, b)`); a target is a
                // method-style call (`a.f(b)`) the native AST cannot express.
                if call.target.is_some() {
                    return Err(ConversionError::UnsupportedExpr("method-style call"));
                }
                let args = call
                    .args
                    .into_iter()
                    .map(Expr::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr::Call {
                    function: call.function,
                    args,
                })
            }
            Some(ExprKind::ListExpr(_)) => Err(ConversionError::UnsupportedExpr("list creation")),
            Some(ExprKind::StructExpr(_)) => {
                Err(ConversionError::UnsupportedExpr("struct creation"))
            }
            Some(ExprKind::ComprehensionExpr(_)) => {
                Err(ConversionError::UnsupportedExpr("comprehension"))
            }
            None => Err(ConversionError::Missing("expression kind")),
        }
    }
}

impl TryFrom<cel::CheckedExpr> for Filter {
    type Error = ConversionError;

    fn try_from(checked: cel::CheckedExpr) -> Result<Self, Self::Error> {
        let expr = checked
            .expr
            .ok_or(ConversionError::Missing("checked expr"))?;
        Ok(Filter {
            expr: expr.try_into()?,
        })
    }
}
