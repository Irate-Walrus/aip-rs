//! Walking the AIP-160 [`Filter`] AST into a [`Predicate`].

use aip_filtering::{function, Constant, Declarations, Expr, Filter, Type};

use crate::predicate::{CmpOp, Column, Predicate, Value};
use crate::schema::Schema;
use crate::Error;

/// Transpile a type-checked [`Filter`] into a [`Predicate`].
///
/// `schema` maps each filter identifier to its SQL column (and so is the
/// allowlist of filterable columns); `declarations` is the same set the filter
/// was [checked](aip_filtering::check) against, used to recover the operand
/// types `check` discards — notably to tell a bare enum *value* (bound as its
/// name) apart from a declared column missing from `schema`.
///
/// Every literal becomes a bound [`Value`] — nothing is spliced into SQL text
/// (ADR-0005 / ADR-0008). Constructs outside the comparison/logical operator set
/// (the has operator `:`, a comparison between two columns) are
/// [`Error::Unsupported`].
pub fn transpile_filter(
    filter: &Filter,
    declarations: &Declarations,
    schema: &Schema,
) -> Result<Predicate, Error> {
    let ctx = Ctx {
        declarations,
        schema,
    };
    transpile_expr(&filter.expr, &ctx)
}

/// The declarations + column schema a transpile walk reads, bundled so the
/// recursive helpers don't thread two references through every call.
struct Ctx<'a> {
    declarations: &'a Declarations,
    schema: &'a Schema,
}

/// Transpile one boolean expression node. A type-checked filter is rooted at a
/// boolean call, so anything else here is unsupported.
fn transpile_expr(expr: &Expr, ctx: &Ctx) -> Result<Predicate, Error> {
    match expr {
        Expr::Call { function, args } => transpile_call(function, args, ctx),
        other => Err(Error::Unsupported(format!(
            "expected a boolean expression, found {other:?}"
        ))),
    }
}

/// Transpile a call node: `AND` / `OR` / `NOT` lower to the boolean [`Predicate`]
/// combinators; each comparison operator to a [`Predicate::Compare`] leaf.
fn transpile_call(name: &str, args: &[Expr], ctx: &Ctx) -> Result<Predicate, Error> {
    match (name, args) {
        (function::AND, [left, right]) => Ok(Predicate::all([
            transpile_expr(left, ctx)?,
            transpile_expr(right, ctx)?,
        ])),
        (function::OR, [left, right]) => Ok(Predicate::any([
            transpile_expr(left, ctx)?,
            transpile_expr(right, ctx)?,
        ])),
        (function::NOT, [inner]) => Ok(Predicate::not(transpile_expr(inner, ctx)?)),
        (function::EQUALS, [left, right]) => transpile_compare(CmpOp::Eq, left, right, ctx),
        (function::NOT_EQUALS, [left, right]) => transpile_compare(CmpOp::Ne, left, right, ctx),
        (function::LESS_THAN, [left, right]) => transpile_compare(CmpOp::Lt, left, right, ctx),
        (function::LESS_EQUALS, [left, right]) => transpile_compare(CmpOp::Le, left, right, ctx),
        (function::GREATER_THAN, [left, right]) => transpile_compare(CmpOp::Gt, left, right, ctx),
        (function::GREATER_EQUALS, [left, right]) => transpile_compare(CmpOp::Ge, left, right, ctx),
        // Recognized binary operators applied with the wrong arity.
        (
            function::AND
            | function::OR
            | function::EQUALS
            | function::NOT_EQUALS
            | function::LESS_THAN
            | function::LESS_EQUALS
            | function::GREATER_THAN
            | function::GREATER_EQUALS,
            _,
        ) => Err(Error::Unsupported(format!(
            "`{name}` expects two operands, found {}",
            args.len()
        ))),
        (function::NOT, _) => Err(Error::Unsupported(format!(
            "`NOT` expects one operand, found {}",
            args.len()
        ))),
        // The has operator `:` (#41), the implicit-AND `FUZZY`, a bare
        // `timestamp(...)` used as a predicate, etc.
        _ => Err(Error::Unsupported(format!("operator `{name}`"))),
    }
}

/// Transpile a comparison: classify each operand as a column or a bound value,
/// then emit `column <op> value`, flipping the operator if the column sat on the
/// right (`"x" < region` becomes `region > "x"`).
fn transpile_compare(op: CmpOp, left: &Expr, right: &Expr, ctx: &Ctx) -> Result<Predicate, Error> {
    match (classify(left, ctx)?, classify(right, ctx)?) {
        (Operand::Column(column), Operand::Value(value)) => {
            Ok(Predicate::Compare { column, op, value })
        }
        (Operand::Value(value), Operand::Column(column)) => Ok(Predicate::Compare {
            column,
            op: op.mirror(),
            value,
        }),
        (Operand::Column(_), Operand::Column(_)) => Err(Error::Unsupported(
            "a comparison between two columns is not supported".to_string(),
        )),
        (Operand::Value(_), Operand::Value(_)) => Err(Error::Unsupported(
            "a comparison must reference a filterable column".to_string(),
        )),
    }
}

/// One side of a comparison, resolved to either a SQL column or a bound value.
enum Operand {
    Column(Column),
    Value(Value),
}

/// Resolve one comparison operand to a column or a bound value.
fn classify(expr: &Expr, ctx: &Ctx) -> Result<Operand, Error> {
    match expr {
        // An identifier is a column when the schema maps it; otherwise it must be
        // a bare enum value (declared an `Enum` ident), bound as its name. A
        // declared non-enum identifier missing from the schema is an unmapped
        // column — a clearer error than silently treating it as a value.
        Expr::Ident(name) => match ctx.schema.column(name) {
            Some(column) => Ok(Operand::Column(Column::Plain(column.to_string()))),
            None => match ctx.declarations.ident_type(name) {
                Some(Type::Enum(_)) => Ok(Operand::Value(Value::Text(name.clone()))),
                _ => Err(Error::UnknownIdentifier(name.clone())),
            },
        },
        Expr::Const(constant) => Ok(Operand::Value(to_value(constant))),
        Expr::Call { function, args } => classify_constructor(function, args),
        Expr::Select { .. } => classify_select(expr, ctx),
    }
}

/// Resolve a `timestamp(...)` / `duration(...)` constructor — the only calls
/// valid in operand position — to a bound value. A timestamp lifts its RFC3339
/// string straight to a text bind (the column stores RFC3339 text); a duration
/// is normalized to its total seconds so it compares numerically (ADR-0008
/// amendment #40).
fn classify_constructor(name: &str, args: &[Expr]) -> Result<Operand, Error> {
    match (name, args) {
        (function::TIMESTAMP, [Expr::Const(Constant::String(text))]) => {
            Ok(Operand::Value(Value::Text(text.clone())))
        }
        (function::DURATION, [Expr::Const(Constant::String(text))]) => {
            Ok(Operand::Value(Value::Double(parse_duration_seconds(text)?)))
        }
        _ => Err(Error::Unsupported(format!(
            "`{name}` is not valid in comparison operand position"
        ))),
    }
}

/// Resolve a member-selection operand. A fully-qualified path declared as one
/// column (`lat_lng.latitude`) maps straight to that column; otherwise the base
/// must be a `map` column and the trailing field is the key (`labels.env`) —
/// mirroring the checker's resolution of a [`Select`](Expr::Select).
fn classify_select(expr: &Expr, ctx: &Ctx) -> Result<Operand, Error> {
    if let Some(qualified) = qualified_name(expr) {
        if let Some(column) = ctx.schema.column(&qualified) {
            return Ok(Operand::Column(Column::Plain(column.to_string())));
        }
    }
    if let Expr::Select { operand, field } = expr {
        if let Expr::Ident(base) = operand.as_ref() {
            if let Some(column) = ctx.schema.column(base) {
                return Ok(Operand::Column(Column::MapMember {
                    column: column.to_string(),
                    key: field.clone(),
                }));
            }
        }
    }
    Err(Error::UnknownIdentifier(
        qualified_name(expr).unwrap_or_else(|| "<member>".to_string()),
    ))
}

/// The dotted name of an identifier/selection chain (`lat_lng.latitude`), or
/// `None` if it is not a plain identifier path.
fn qualified_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name) => Some(name.clone()),
        Expr::Select { operand, field } => Some(format!("{}.{field}", qualified_name(operand)?)),
        _ => None,
    }
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

/// Parse a protobuf duration literal — a number of seconds suffixed with `s`,
/// e.g. `"3600s"` or `"1.5s"` — into its total seconds.
fn parse_duration_seconds(literal: &str) -> Result<f64, Error> {
    literal
        .strip_suffix('s')
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .ok_or_else(|| Error::InvalidDuration(literal.to_string()))
}
