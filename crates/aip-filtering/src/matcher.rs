//! In-memory evaluation of a checked [`Filter`] against a message.
//!
//! Where [`aip-sql`] *transpiles* a [`Filter`] into SQL for a database to run,
//! the matcher *evaluates* it directly — walking a message through its
//! [`Descriptor`](prost_reflect::ReflectMessage::descriptor) and deciding whether
//! it matches. It completes the AIP-160 story for callers without a database,
//! enables post-filtering, and is the reference the SQL path is checked against:
//! the same [`Filter`] must select the same messages in memory and in SQLite.
//!
//! Presented as a **Typed facade** ([`matches()`], generic over
//! [`ReflectMessage`] so a message carries its own descriptor) layered on a
//! still-public **Dynamic core** ([`matches_dynamic`], over [`DynamicMessage`]),
//! exactly as `aip-fieldmask` layers `update` on `update_dynamic`
//! (`docs/adr/0009-reflective-typed-message-api.md`).
//!
//! ## Agreement with the SQL path
//!
//! The lowering of each operand mirrors `aip-sql` (ADR-0008 amendments #40/#41),
//! so a matcher verdict and a SQLite `WHERE` agree:
//!
//! - **Enum** `=`/`!=` compares the value *name*, not its number.
//! - **Timestamp** comparisons lower the field to a canonical, second-precision
//!   RFC3339 UTC string and compare it **lexicographically** against the literal
//!   — the sortable text form ADR-0008 expects the column to store, so no
//!   timestamp parsing is needed and the in-memory order matches SQL's.
//! - **Duration** literals lower to their total seconds and compare numerically.
//! - The has operator `:` substring is **ASCII case-insensitive**, matching
//!   SQLite's default `LIKE`; map-key / list-element test membership; a timestamp
//!   `field:*` tests presence.
//! - An absent operand (an unset singular message, or a missing map key) makes a
//!   comparison **unknown**, propagated through `AND`/`OR`/`NOT` as SQL's
//!   three-valued logic; a message matches only when the whole filter is *true*
//!   (mirroring a SQL `WHERE`, where `NULL` and `false` both exclude the row).
//!
//! [`aip-sql`]: https://docs.rs/aip-sql
//! [`Filter`]: crate::Filter

use std::cmp::Ordering;

use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, MapKey, ReflectMessage, Value};

use crate::{function, Constant, Declarations, Expr, Filter, Type};

/// Errors evaluating a checked [`Filter`] against a message.
///
/// A [`Filter`] that passed [`check`](crate::check) against the same
/// [`Declarations`] is well-typed, so these signal a construct outside the
/// evaluable set rather than bad user input — the matcher's analogue of
/// `aip-sql`'s transpile errors.
#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    /// A construct outside the comparison / logical / has-operator set the
    /// checker accepts — e.g. a comparison between two message fields.
    #[error("unsupported filter construct: {0}")]
    Unsupported(String),
    /// An identifier that is neither a field of the message nor a declared enum
    /// value — a [`Declarations`]/message mismatch, the analogue of `aip-sql`'s
    /// unmapped column.
    #[error("identifier `{0}` is not a field of the message nor a declared value")]
    UnknownIdentifier(String),
    /// A `duration(...)` literal that is not a number of seconds suffixed with
    /// `s` (e.g. `"3600s"`).
    #[error("invalid duration literal: {0}")]
    InvalidDuration(String),
}

/// Evaluates a checked [`Filter`] against a **Typed message**, reporting whether
/// it matches. The headline interface over the [`matches_dynamic`] core
/// (ADR-0009).
///
/// `message` is a concrete generated message that carries its own
/// [`Descriptor`](ReflectMessage::descriptor); the facade transcodes it to a
/// [`DynamicMessage`] through its wire bytes and runs the core. The matcher only
/// reads `message`, so — like [`request_checksum`](crate) — it needs no
/// `Default` and decodes nothing back; the `M → DynamicMessage` transcode can
/// fail only if a type and its descriptor disagree (a build bug, not bad input),
/// so it is an invariant (`expect`), not an [`Error`](MatchError) variant.
///
/// `declarations` is the same set the filter was [checked](crate::check)
/// against; it is consulted only to recognise a bare enum *value* identifier
/// (`state = ACTIVE`), exactly as `aip-sql` recovers one.
pub fn matches<M: ReflectMessage>(
    filter: &Filter,
    declarations: &Declarations,
    message: &M,
) -> Result<bool, MatchError> {
    let dynamic = DynamicMessage::decode(message.descriptor(), message.encode_to_vec().as_slice())
        .expect("a message round-trips through its own descriptor");
    matches_dynamic(filter, declarations, &dynamic)
}

/// The **Dynamic core** of the matcher: evaluates a checked [`Filter`] against a
/// [`DynamicMessage`] (ADR-0009).
///
/// The low-level interface over [`DynamicMessage`] — the escape hatch for a
/// caller who already holds a dynamic message (JSON ingestion, a generic
/// gateway) and the crate's test surface. Callers holding concrete generated
/// types reach for the [`matches()`] **Typed facade**, which transcodes onto this.
///
/// A message matches when the filter evaluates to *true*; an *unknown* result
/// (a comparison against an unset field) excludes it, the same way a SQL `WHERE`
/// excludes a row whose predicate is `NULL`.
pub fn matches_dynamic(
    filter: &Filter,
    declarations: &Declarations,
    message: &DynamicMessage,
) -> Result<bool, MatchError> {
    let ctx = Ctx {
        declarations,
        message,
    };
    Ok(eval(&filter.expr, &ctx)? == Truth::True)
}

/// The declarations + message an evaluation walk reads, bundled so the recursive
/// helpers don't thread two references through every call.
struct Ctx<'a> {
    declarations: &'a Declarations,
    message: &'a DynamicMessage,
}

/// A three-valued truth, matching SQL's logic so the matcher agrees with a
/// SQLite `WHERE` over fields that may be unset (`NULL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    /// `true`/`false` to definite [`True`](Truth::True)/[`False`](Truth::False).
    fn known(value: bool) -> Self {
        if value {
            Truth::True
        } else {
            Truth::False
        }
    }
}

/// A comparable, lowered operand value. Both sides of a comparison lower to one
/// of these — mirroring `aip-sql`'s bind values — so an enum compares by name, a
/// timestamp by its RFC3339 text, and a duration by its seconds.
#[derive(Debug, Clone, PartialEq)]
enum Operand {
    /// A numeric operand: an int, uint, double, or a duration's total seconds.
    Num(f64),
    /// A textual operand: a string, an enum value name, or a timestamp's RFC3339
    /// rendering — compared lexicographically.
    Text(String),
    /// A boolean operand (`=`/`!=` only).
    Bool(bool),
}

/// One side of a comparison, classified as a message field or a literal value —
/// the matcher's analogue of `aip-sql`'s `classify`.
enum Side {
    /// A reference to a message field, resolved to its value (or absence).
    Field(FieldValue),
    /// A literal: a constant, a `timestamp(...)`/`duration(...)` constructor, or
    /// a bare enum value identifier.
    Literal,
}

/// A resolved field operand: present with its descriptor and value, or absent
/// (an unset singular message / a missing map key → an *unknown* comparison).
enum FieldValue {
    /// A present field, carrying its descriptor (needed to name an enum value).
    Present(FieldDescriptor, Value),
    /// A field whose value is unset (an unset singular message field, or a
    /// missing map key) — a comparison against it is [`Unknown`](Truth::Unknown).
    Absent,
}

/// Evaluate one boolean expression node. A checked filter is rooted at a boolean
/// call, so anything else here is unsupported.
fn eval(expr: &Expr, ctx: &Ctx) -> Result<Truth, MatchError> {
    match expr {
        Expr::Call { function, args } => eval_call(function, args, ctx),
        other => Err(MatchError::Unsupported(format!(
            "expected a boolean expression, found {other:?}"
        ))),
    }
}

/// Evaluate a call node: `AND`/`OR`/`NOT` combine sub-results in three-valued
/// logic; each comparison and the has operator lower to a leaf truth.
fn eval_call(name: &str, args: &[Expr], ctx: &Ctx) -> Result<Truth, MatchError> {
    match (name, args) {
        (function::AND, [left, right]) => Ok(and(eval(left, ctx)?, eval(right, ctx)?)),
        (function::OR, [left, right]) => Ok(or(eval(left, ctx)?, eval(right, ctx)?)),
        (function::NOT, [inner]) => Ok(not(eval(inner, ctx)?)),
        (function::EQUALS, [left, right]) => compare(CmpOp::Eq, left, right, ctx),
        (function::NOT_EQUALS, [left, right]) => compare(CmpOp::Ne, left, right, ctx),
        (function::LESS_THAN, [left, right]) => compare(CmpOp::Lt, left, right, ctx),
        (function::LESS_EQUALS, [left, right]) => compare(CmpOp::Le, left, right, ctx),
        (function::GREATER_THAN, [left, right]) => compare(CmpOp::Gt, left, right, ctx),
        (function::GREATER_EQUALS, [left, right]) => compare(CmpOp::Ge, left, right, ctx),
        (function::HAS, [left, right]) => has(left, right, ctx),
        _ => Err(MatchError::Unsupported(format!("operator `{name}`"))),
    }
}

/// Conjunction in three-valued logic: false if either side is false, true only
/// when both are true, otherwise unknown.
fn and(left: Truth, right: Truth) -> Truth {
    match (left, right) {
        (Truth::False, _) | (_, Truth::False) => Truth::False,
        (Truth::True, Truth::True) => Truth::True,
        _ => Truth::Unknown,
    }
}

/// Disjunction in three-valued logic: true if either side is true, false only
/// when both are false, otherwise unknown.
fn or(left: Truth, right: Truth) -> Truth {
    match (left, right) {
        (Truth::True, _) | (_, Truth::True) => Truth::True,
        (Truth::False, Truth::False) => Truth::False,
        _ => Truth::Unknown,
    }
}

/// Negation in three-valued logic: unknown negates to unknown.
fn not(value: Truth) -> Truth {
    match value {
        Truth::True => Truth::False,
        Truth::False => Truth::True,
        Truth::Unknown => Truth::Unknown,
    }
}

/// A comparison operator, with the mirror used to canonicalise a
/// `value <op> field` comparison into `field <op> value`.
#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    /// This operator with its operands swapped (`"x" < field` → `field > "x"`).
    fn mirror(self) -> CmpOp {
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

/// Evaluate a comparison: classify each operand as a field or a literal, then
/// evaluate `field <op> value`, mirroring the operator if the field sat on the
/// right (so `"x" < region` becomes `region > "x"`).
fn compare(op: CmpOp, left: &Expr, right: &Expr, ctx: &Ctx) -> Result<Truth, MatchError> {
    match (classify(left, ctx)?, classify(right, ctx)?) {
        (Side::Field(field), Side::Literal) => eval_compare(op, field, right),
        (Side::Literal, Side::Field(field)) => eval_compare(op.mirror(), field, left),
        (Side::Field(_), Side::Field(_)) => Err(MatchError::Unsupported(
            "a comparison between two fields is not supported".to_string(),
        )),
        (Side::Literal, Side::Literal) => Err(MatchError::Unsupported(
            "a comparison must reference a message field".to_string(),
        )),
    }
}

/// Evaluate `field <op> literal`. An absent field makes the comparison unknown;
/// otherwise both sides lower to a comparable [`Operand`] and the operator is
/// applied.
fn eval_compare(op: CmpOp, field: FieldValue, literal: &Expr) -> Result<Truth, MatchError> {
    let field_operand = match field {
        FieldValue::Absent => return Ok(Truth::Unknown),
        FieldValue::Present(descriptor, value) => lower_field(&descriptor, &value)?,
    };
    let literal_operand = lower_literal(literal)?;
    apply(op, &field_operand, &literal_operand)
}

/// Apply a comparison operator to two lowered operands. Operands of different
/// kinds cannot be ordered — a checker violation, so it is unsupported.
fn apply(op: CmpOp, left: &Operand, right: &Operand) -> Result<Truth, MatchError> {
    let ordering = match (left, right) {
        (Operand::Num(a), Operand::Num(b)) => a.partial_cmp(b),
        (Operand::Text(a), Operand::Text(b)) => Some(a.cmp(b)),
        (Operand::Bool(a), Operand::Bool(b)) => Some(a.cmp(b)),
        _ => {
            return Err(MatchError::Unsupported(
                "a comparison between mismatched operand types".to_string(),
            ))
        }
    };
    // A `NaN` operand orders against nothing, so every comparison with it is
    // false (it never appears in this demo's data, but keeps `apply` total).
    let Some(ordering) = ordering else {
        return Ok(Truth::False);
    };
    let matched = match op {
        CmpOp::Eq => ordering == Ordering::Equal,
        CmpOp::Ne => ordering != Ordering::Equal,
        CmpOp::Lt => ordering == Ordering::Less,
        CmpOp::Le => ordering != Ordering::Greater,
        CmpOp::Gt => ordering == Ordering::Greater,
        CmpOp::Ge => ordering != Ordering::Less,
    };
    Ok(Truth::known(matched))
}

/// Evaluate the has operator `:` (AIP-160 presence/membership). The left operand
/// names the field under test; its reflected kind selects the overload — a
/// substring of a string, key presence in a map, element presence in a list, or
/// presence of a timestamp (`field:*`). The right operand is always a string the
/// parser lifted from the filter.
fn has(left: &Expr, right: &Expr, ctx: &Ctx) -> Result<Truth, MatchError> {
    let Expr::Const(Constant::String(arg)) = right else {
        return Err(MatchError::Unsupported(
            "the has operator's right operand must be a string value".to_string(),
        ));
    };

    let (descriptor, value) = match classify(left, ctx)? {
        // Only a singular *message* field can be absent (a proto3 string / map /
        // list always holds a value), and the only message overload of `:` is a
        // timestamp presence test — so an absent field is simply "not present".
        Side::Field(FieldValue::Absent) => return Ok(Truth::False),
        Side::Field(FieldValue::Present(descriptor, value)) => (descriptor, value),
        Side::Literal => {
            return Err(MatchError::Unsupported(
                "the has operator's left operand must be a message field".to_string(),
            ))
        }
    };

    if descriptor.is_map() {
        // Map-key presence (`map:key`): the key is present in the stored map.
        let present = value
            .as_map()
            .is_some_and(|map| map.contains_key(&MapKey::String(arg.clone())));
        Ok(Truth::known(present))
    } else if descriptor.is_list() {
        // List-element membership (`list:value`): the value is one of the elements.
        let present = value
            .as_list()
            .is_some_and(|list| list.iter().any(|item| item.as_str() == Some(arg.as_str())));
        Ok(Truth::known(present))
    } else {
        match descriptor.kind() {
            // Substring (`field:value`): ASCII case-insensitive containment,
            // matching SQLite's default `LIKE` so the matcher and SQL agree.
            Kind::String => {
                let haystack = value.as_str().unwrap_or_default().to_ascii_lowercase();
                Ok(Truth::known(haystack.contains(&arg.to_ascii_lowercase())))
            }
            // Timestamp presence (`field:*`): reaching here means the field was
            // present (an absent one returned above), so it is present.
            Kind::Message(message) if message.full_name() == "google.protobuf.Timestamp" => {
                Ok(Truth::True)
            }
            other => Err(MatchError::Unsupported(format!(
                "the has operator does not apply to a {other:?} field"
            ))),
        }
    }
}

/// Classify one comparison/has operand as a message field or a literal —
/// mirroring `aip-sql`'s `classify`/`classify_select`.
fn classify(expr: &Expr, ctx: &Ctx) -> Result<Side, MatchError> {
    match expr {
        Expr::Const(_) => Ok(Side::Literal),
        // `timestamp(...)` / `duration(...)` are the only calls valid in operand
        // position; both are literals (handled by `lower_literal`).
        Expr::Call { function, .. }
            if function == function::TIMESTAMP || function == function::DURATION =>
        {
            Ok(Side::Literal)
        }
        Expr::Call { function, .. } => Err(MatchError::Unsupported(format!(
            "`{function}` is not valid in comparison operand position"
        ))),
        // A bare identifier is a field when the message has it; otherwise it must
        // be a declared enum value (a literal), else it is unmapped.
        Expr::Ident(name) => match resolve_path(ctx.message, &[name.as_str()]) {
            PathResult::NotAField => match ctx.declarations.ident_type(name) {
                Some(Type::Enum(_)) => Ok(Side::Literal),
                _ => Err(MatchError::UnknownIdentifier(name.clone())),
            },
            resolved => Ok(Side::Field(resolved.into_field())),
        },
        Expr::Select { .. } => classify_select(expr, ctx),
    }
}

/// Resolve a member-selection operand. A fully-qualified path that resolves
/// field-by-field (`lat_lng.latitude`) is that field; otherwise the base must be
/// a map and the trailing segment its key (`labels.env`) — mirroring the
/// checker's resolution of a [`Select`](Expr::Select) and `aip-sql`'s.
fn classify_select(expr: &Expr, ctx: &Ctx) -> Result<Side, MatchError> {
    if let Some(segments) = qualified_segments(expr) {
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        if let resolved @ (PathResult::Present(..) | PathResult::Absent) =
            resolve_path(ctx.message, &refs)
        {
            return Ok(Side::Field(resolved.into_field()));
        }
    }
    // Member access into a `map<string,string>` field: `base.key` reads the
    // value stored at `key`, or is absent (→ unknown) when the key is missing.
    if let Expr::Select { operand, field } = expr {
        if let Expr::Ident(base) = operand.as_ref() {
            if let Some(descriptor) = ctx.message.descriptor().get_field_by_name(base) {
                if descriptor.is_map() {
                    let value = ctx.message.get_field(&descriptor);
                    let member = value
                        .as_map()
                        .and_then(|map| map.get(&MapKey::String(field.clone())))
                        .cloned();
                    return Ok(Side::Field(match member {
                        Some(member) => FieldValue::Present(map_value_field(&descriptor), member),
                        None => FieldValue::Absent,
                    }));
                }
            }
        }
    }
    Err(MatchError::UnknownIdentifier(
        qualified_segments(expr)
            .map(|segments| segments.join("."))
            .unwrap_or_else(|| "<member>".to_string()),
    ))
}

/// The outcome of resolving a dotted field path against a message.
enum PathResult {
    /// The path resolved to a present field, with its descriptor and value.
    Present(FieldDescriptor, Value),
    /// The path is valid but a singular message along it (or the leaf) is unset.
    Absent,
    /// The path does not resolve field-by-field (an unknown field, or one
    /// descending through a map/list/scalar) — possibly a map member access.
    NotAField,
}

impl PathResult {
    /// Collapse a resolved path into a [`FieldValue`]; [`NotAField`](Self::NotAField)
    /// is only reached after the caller has ruled it out, so it folds to absent.
    fn into_field(self) -> FieldValue {
        match self {
            PathResult::Present(descriptor, value) => FieldValue::Present(descriptor, value),
            PathResult::Absent | PathResult::NotAField => FieldValue::Absent,
        }
    }
}

/// Walk a dotted field path through `message`, descending one singular message
/// per non-leaf segment.
///
/// Presence is meaningful only at a singular *message* boundary: an unset
/// message field makes the whole path [`Absent`](PathResult::Absent) (the SQL
/// column would be `NULL`), while a proto3 scalar/enum/string/repeated/map leaf
/// always holds a value (its default), so it is always
/// [`Present`](PathResult::Present) — matching the non-null columns the store
/// writes for them.
fn resolve_path(message: &DynamicMessage, segments: &[&str]) -> PathResult {
    let Some((segment, rest)) = segments.split_first() else {
        return PathResult::NotAField;
    };
    let Some(field) = message.descriptor().get_field_by_name(segment) else {
        return PathResult::NotAField;
    };

    if rest.is_empty() {
        // A singular message leaf has presence; an unset one is absent.
        if is_singular_message(&field) && !message.has_field(&field) {
            return PathResult::Absent;
        }
        return PathResult::Present(field.clone(), message.get_field(&field).into_owned());
    }

    // An interior segment must descend through a singular message.
    if !is_singular_message(&field) {
        return PathResult::NotAField;
    }
    if !message.has_field(&field) {
        return PathResult::Absent;
    }
    let value = message.get_field(&field);
    let submessage = value
        .as_message()
        .expect("a singular message field holds a message value");
    resolve_path(submessage, rest)
}

/// Whether `field` is a singular message field — the only kind with proto3
/// presence (so the only kind that can be [`Absent`](PathResult::Absent)).
fn is_singular_message(field: &FieldDescriptor) -> bool {
    matches!(field.kind(), Kind::Message(_)) && !field.is_list() && !field.is_map()
}

/// The value-field descriptor of a `map<_, _>` field — the descriptor a
/// member-access value should carry, so an enum-valued map lowers by name like
/// any other enum.
fn map_value_field(map_field: &FieldDescriptor) -> FieldDescriptor {
    match map_field.kind() {
        Kind::Message(entry) => entry.map_entry_value_field(),
        // A map field is always a message (its entry type), so this is unreachable
        // for a real map; return the field itself to keep the helper total.
        _ => map_field.clone(),
    }
}

/// Lower a present field value to a comparable [`Operand`], mirroring `aip-sql`'s
/// bind-value lowering: an enum to its value *name*, a timestamp to RFC3339 text,
/// a duration to its total seconds.
///
/// Integers widen into the `f64` [`Num`](Operand::Num) so a mixed `double`/`int`
/// comparison has one numeric type — exact up to 2^53, like a JSON number. A
/// 64-bit id beyond that loses precision (and a `u64` above `i64::MAX` is widened
/// the same way `aip-sql` narrows it into a signed bind, lossily); the demo's
/// filterable numbers (a latitude, small ids) stay well inside the exact range.
fn lower_field(descriptor: &FieldDescriptor, value: &Value) -> Result<Operand, MatchError> {
    Ok(match value {
        Value::Bool(boolean) => Operand::Bool(*boolean),
        Value::I32(int) => Operand::Num(*int as f64),
        Value::I64(int) => Operand::Num(*int as f64),
        Value::U32(uint) => Operand::Num(*uint as f64),
        Value::U64(uint) => Operand::Num(*uint as f64),
        Value::F32(float) => Operand::Num(*float as f64),
        Value::F64(float) => Operand::Num(*float),
        Value::String(string) => Operand::Text(string.clone()),
        Value::EnumNumber(number) => {
            let Kind::Enum(enum_descriptor) = descriptor.kind() else {
                return Err(MatchError::Unsupported(
                    "an enum value on a non-enum field".to_string(),
                ));
            };
            // The value name, mirroring `as_str_name()` / the SQL enum binding; an
            // unrecognised number falls back to its decimal text.
            let name = enum_descriptor
                .get_value(*number)
                .map(|value| value.name().to_string())
                .unwrap_or_else(|| number.to_string());
            Operand::Text(name)
        }
        Value::Message(message) => match message.descriptor().full_name() {
            "google.protobuf.Timestamp" => Operand::Text(rfc3339(message)),
            "google.protobuf.Duration" => Operand::Num(duration_seconds(message)),
            other => {
                return Err(MatchError::Unsupported(format!(
                    "cannot compare a {other} message field"
                )))
            }
        },
        Value::Bytes(_) | Value::List(_) | Value::Map(_) => {
            return Err(MatchError::Unsupported(
                "cannot compare a bytes, list, or map field".to_string(),
            ))
        }
    })
}

/// Lower a literal operand — a constant, a `timestamp(...)`/`duration(...)`
/// constructor, or a bare enum value identifier — to a comparable [`Operand`].
fn lower_literal(expr: &Expr) -> Result<Operand, MatchError> {
    match expr {
        Expr::Const(Constant::Int(int)) => Ok(Operand::Num(*int as f64)),
        Expr::Const(Constant::Uint(uint)) => Ok(Operand::Num(*uint as f64)),
        Expr::Const(Constant::Double(double)) => Ok(Operand::Num(*double)),
        Expr::Const(Constant::Bool(boolean)) => Ok(Operand::Bool(*boolean)),
        Expr::Const(Constant::String(string)) => Ok(Operand::Text(string.clone())),
        // A bare enum value identifier (`ACTIVE`) compares by its name.
        Expr::Ident(name) => Ok(Operand::Text(name.clone())),
        Expr::Call { function, args } => match (function.as_str(), args.as_slice()) {
            // A timestamp literal binds as its RFC3339 text; a duration as its
            // total seconds — exactly as `aip-sql` lowers them.
            (function::TIMESTAMP, [Expr::Const(Constant::String(text))]) => {
                Ok(Operand::Text(text.clone()))
            }
            (function::DURATION, [Expr::Const(Constant::String(text))]) => {
                Ok(Operand::Num(duration_seconds_str(text)?))
            }
            _ => Err(MatchError::Unsupported(format!(
                "`{function}` is not valid in comparison operand position"
            ))),
        },
        Expr::Const(Constant::Bytes(_)) => Err(MatchError::Unsupported(
            "a bytes literal is not comparable".to_string(),
        )),
        Expr::Select { .. } => Err(MatchError::Unsupported(
            "a member selection is not a literal".to_string(),
        )),
    }
}

/// The dotted segments of an identifier/selection chain (`lat_lng.latitude` →
/// `["lat_lng", "latitude"]`), or `None` if it is not a plain identifier path.
fn qualified_segments(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(name) => Some(vec![name.clone()]),
        Expr::Select { operand, field } => {
            let mut segments = qualified_segments(operand)?;
            segments.push(field.clone());
            Some(segments)
        }
        _ => None,
    }
}

/// Format a `google.protobuf.Timestamp` message as a canonical, second-precision
/// RFC3339 UTC string — the sortable text form ADR-0008 expects a timestamp
/// column to store, so a lexicographic comparison here matches the SQL one. The
/// civil date is Howard Hinnant's `civil_from_days` algorithm; sub-second `nanos`
/// are dropped, as the store's `rfc3339` drops them.
fn rfc3339(message: &DynamicMessage) -> String {
    let seconds = message
        .get_field_by_name("seconds")
        .and_then(|value| value.as_i64())
        .unwrap_or(0)
        .max(0);
    let days = seconds.div_euclid(86_400);
    let tod = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The total seconds of a `google.protobuf.Duration` message — the numeric form
/// `aip-sql` binds a `duration(...)` literal as, so the two compare alike.
fn duration_seconds(message: &DynamicMessage) -> f64 {
    let seconds = message
        .get_field_by_name("seconds")
        .and_then(|value| value.as_i64())
        .unwrap_or(0) as f64;
    let nanos = message
        .get_field_by_name("nanos")
        .and_then(|value| value.as_i32())
        .unwrap_or(0) as f64;
    seconds + nanos / 1_000_000_000.0
}

/// Parse a protobuf duration literal — a number of seconds suffixed with `s`,
/// e.g. `"3600s"` or `"1.5s"` — into its total seconds, mirroring `aip-sql`.
fn duration_seconds_str(literal: &str) -> Result<f64, MatchError> {
    literal
        .strip_suffix('s')
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .ok_or_else(|| MatchError::InvalidDuration(literal.to_string()))
}
