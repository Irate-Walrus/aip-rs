//! AIP-160 filtering: parse and type-check filter expressions into a native AST.
//!
//! The AST is a native Rust enum (not the CEL proto) — it's filtering's primary
//! product, so it's built to be walked. Optional CEL-proto interop lives behind
//! the `cel-proto` feature.
//!
//! Declarations are explicit (an allowlist of filterable identifiers and
//! functions); the parse and check core is reflection-free, with `enum_ident`
//! the one reflective hook.
//!
//! See <https://google.aip.dev/160>.
#![cfg_attr(docsrs, feature(doc_cfg))]

use std::collections::HashMap;

use prost_reflect::{EnumDescriptor, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage};

mod checker;
mod lexer;
mod macros;
mod matcher;
mod parser;
mod token;

pub use macros::{apply_macros, Cursor};
pub use matcher::{matches, matches_dynamic, MatchError};

/// The standard AIP-160 function and operator names that the [parser](parse)
/// emits and the checker resolves. Walk a [`Filter`] by matching
/// [`Expr::Call`]'s `function` against these.
pub mod function {
    /// Logical conjunction (`a AND b`).
    pub const AND: &str = "AND";
    /// Logical disjunction (`a OR b`).
    pub const OR: &str = "OR";
    /// Logical negation (`NOT a` / `-a`).
    pub const NOT: &str = "NOT";
    /// Implicit conjunction between space-separated terms (`a b`). Deliberately
    /// not part of [`standard_functions`](crate::DeclarationsBuilder::standard_functions):
    /// a caller wanting implicit-AND semantics must declare it themselves.
    pub const FUZZY: &str = "FUZZY";
    /// Equality (`a = b`).
    pub const EQUALS: &str = "=";
    /// Inequality (`a != b`).
    pub const NOT_EQUALS: &str = "!=";
    /// Less-than (`a < b`).
    pub const LESS_THAN: &str = "<";
    /// Less-than-or-equal (`a <= b`).
    pub const LESS_EQUALS: &str = "<=";
    /// Greater-than (`a > b`).
    pub const GREATER_THAN: &str = ">";
    /// Greater-than-or-equal (`a >= b`).
    pub const GREATER_EQUALS: &str = ">=";
    /// The has operator (`a : b`): presence/membership — a key in a map, a value
    /// in a list, a substring of a string, or `*` presence on a timestamp.
    pub const HAS: &str = ":";
    /// Constructs a timestamp from an RFC3339 string (`timestamp("2024-01-01T00:00:00Z")`).
    pub const TIMESTAMP: &str = "timestamp";
    /// Constructs a duration from a string (`duration("3600s")`).
    pub const DURATION: &str = "duration";
}

/// Errors produced when parsing or type-checking a filter.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("filter syntax error at position {position}: {message}")]
    Syntax { position: usize, message: String },
    #[error("type error: {0}")]
    Type(String),
    #[error("undeclared identifier: {0}")]
    UndeclaredIdent(String),
    #[error("field path '{path}' does not resolve on message {message}")]
    UnknownField { path: String, message: String },
    #[error("field path '{path}' on message {message} has unfilterable type ({detail})")]
    UnfilterableField {
        path: String,
        message: String,
        detail: String,
    },
}

/// The type of a filter expression or declared identifier (CEL-equivalent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Uint,
    Double,
    String,
    Bool,
    Bytes,
    Duration,
    Timestamp,
    Enum(String),
    Message(String),
    Map(Box<Type>, Box<Type>),
    List(Box<Type>),
    Dyn,
    Null,
}

impl Type {
    /// A `map<key, value>` type — the boxing [`Type::Map`] needs, done for you.
    pub fn map(key: Type, value: Type) -> Type {
        Type::Map(Box::new(key), Box::new(value))
    }

    /// A `list<element>` type — the boxing [`Type::List`] needs, done for you.
    pub fn list(element: Type) -> Type {
        Type::List(Box::new(element))
    }
}

/// A literal constant value within a filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Int(i64),
    Uint(u64),
    Double(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
}

/// A node in the native filter AST (mirrors the CEL expression kinds).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal constant.
    Const(Constant),
    /// An identifier reference.
    Ident(String),
    /// Field selection: `operand.field`.
    Select { operand: Box<Expr>, field: String },
    /// Function or operator call (e.g. `=`, `AND`, `NOT`).
    Call { function: String, args: Vec<Expr> },
}

/// A parsed and type-checked filter — filtering's public product.
#[derive(Debug, Clone)]
pub struct Filter {
    /// The type-checked expression tree.
    pub expr: Expr,
}

/// A single overload of a declared function: a result [`Type`] plus the
/// parameter [`Type`]s an argument list must match exactly to resolve it.
#[derive(Debug, Clone)]
pub struct Overload {
    pub(crate) result: Type,
    pub(crate) params: Vec<Type>,
}

impl Overload {
    /// An overload returning `result` for arguments matching `params`.
    pub fn new(result: Type, params: Vec<Type>) -> Self {
        Self { result, params }
    }
}

/// The typed schema a [`Filter`] is checked against: an allowlist of filterable
/// identifiers, plus declared functions and enums.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    /// Declared filterable identifiers, keyed by their (possibly dotted) name.
    idents: HashMap<String, Type>,
    /// Declared functions, keyed by name; each carries its resolvable overloads.
    functions: HashMap<String, Vec<Overload>>,
    /// The declared **field paths**, in declaration order — the subset of
    /// [`idents`](Self::idents) that name an addressable field
    /// (`display_name`, `lat_lng.latitude`, the enum field `state`), *excluding*
    /// the enum *value* names [`enum_ident`](DeclarationsBuilder::enum_ident)
    /// also inserts (`ACTIVE`, …). A downstream column derivation
    /// ([`Schema::for_declarations`](https://docs.rs/aip-sql)) walks these so it
    /// derives one column per declared field and never a bare value name. Both
    /// share the same [`Enum`](Type::Enum) type, so only this membership — not
    /// the type — tells them apart.
    field_paths: Vec<String>,
}

impl Declarations {
    /// Start building a set of declarations.
    pub fn builder() -> DeclarationsBuilder {
        DeclarationsBuilder::default()
    }

    /// Start building declarations whose identifiers are *derived* from the
    /// descriptor of the generated message `M`, via
    /// [`fields`](DeclarationsBuilder::fields).
    ///
    /// The descriptor travels with the Typed message (ADR-0009), so no
    /// descriptor pool is built or threaded — `M::default().descriptor()` is the
    /// reified type the paths resolve against. The explicit
    /// [`ident`](DeclarationsBuilder::ident) /
    /// [`enum_ident`](DeclarationsBuilder::enum_ident) path stays available on
    /// the returned builder for identifiers the descriptor can't supply.
    pub fn for_message<M: ReflectMessage + Default>() -> DeclarationsBuilder {
        DeclarationsBuilder {
            descriptor: Some(M::default().descriptor()),
            ..DeclarationsBuilder::default()
        }
    }

    /// The declared [`Type`] of a filterable identifier, if it was declared.
    ///
    /// [`check`] discards types (it yields only the untyped [`Expr`]), so a
    /// downstream consumer that walks a checked [`Filter`] — e.g. a SQL
    /// transpiler mapping identifiers to columns — recovers an operand's type by
    /// looking it up here rather than re-running the checker. An enum *value*
    /// name (declared by [`enum_ident`](DeclarationsBuilder::enum_ident)) reports
    /// the same [`Enum`](Type::Enum) type as its field, which is how a consumer
    /// tells a bare enum value apart from a missing scalar column.
    pub fn ident_type(&self, name: &str) -> Option<&Type> {
        self.idents.get(name)
    }

    /// The declared **field paths** and their [`Type`]s, in declaration order.
    ///
    /// Yields one entry per addressable field declared by [`ident`] / [`fields`]
    /// / the field side of [`enum_ident`] — `display_name`, `lat_lng.latitude`,
    /// the enum field `state` — and *not* the enum *value* names `enum_ident`
    /// also inserts (`ACTIVE`, …): those are filter literals, not fields, yet
    /// carry the same [`Enum`](Type::Enum) type as their field, so no type test
    /// could separate them. This is the single source a column derivation walks
    /// to map declared fields onto SQL columns without re-listing them by hand.
    ///
    /// [`ident`]: DeclarationsBuilder::ident
    /// [`fields`]: DeclarationsBuilder::fields
    /// [`enum_ident`]: DeclarationsBuilder::enum_ident
    pub fn field_paths(&self) -> impl Iterator<Item = (&str, &Type)> {
        self.field_paths.iter().map(|name| {
            let ty = self
                .idents
                .get(name)
                .expect("a declared field path always has a type");
            (name.as_str(), ty)
        })
    }

    /// Look up a declared function's overloads by name.
    pub(crate) fn lookup_function(&self, name: &str) -> Option<&[Overload]> {
        self.functions.get(name).map(Vec::as_slice)
    }
}

/// Builder for [`Declarations`] (replaces `aip-go`'s functional options).
#[derive(Debug, Default)]
pub struct DeclarationsBuilder {
    idents: HashMap<String, Type>,
    functions: HashMap<String, Vec<Overload>>,
    /// Declared field-path names in declaration order — see
    /// [`Declarations::field_paths`]. Tracked here so [`ident`](Self::ident)
    /// records a field while the private value-name insert in
    /// [`enum_ident`](Self::enum_ident) does not.
    field_paths: Vec<String>,
    /// The message descriptor [`fields`](Self::fields) resolves paths against,
    /// set by [`Declarations::for_message`]. `None` for the explicit builder.
    descriptor: Option<MessageDescriptor>,
    /// The first error a chained derivation hit; surfaced at [`build`](Self::build)
    /// so the builder methods stay infallible and chainable.
    deferred: Option<Error>,
}

impl DeclarationsBuilder {
    /// Declare the standard AIP-160 comparison and logical operators with their
    /// standard overloads: `=` / `!=` (bool, int, double, double/int, string,
    /// timestamp, timestamp/string, duration), the ordering operators
    /// `<` / `<=` / `>` / `>=` (int, double, double/int, string, timestamp,
    /// timestamp/string, duration), `AND` / `OR` / `NOT` over bools, the has
    /// operator `:` (over a string, a `map<string,string>`, a `list<string>`, or
    /// a timestamp), and the `timestamp` / `duration` constructors that lift an
    /// RFC3339 / duration string to a [`Timestamp`](Type::Timestamp) /
    /// [`Duration`](Type::Duration).
    ///
    /// The `timestamp/string` overload lets a timestamp field compare directly
    /// against an RFC3339 literal (`create_time > "2024-01-01T00:00:00Z"`); a
    /// duration string is compared via the `duration` constructor
    /// (`ttl > duration("3600s")`). Enum overloads are per-enum and land with
    /// [`enum_ident`](Self::enum_ident).
    pub fn standard_functions(self) -> Self {
        use Type::{Bool, Double, Duration, Int, String, Timestamp};
        // `=` / `!=` additionally accept two bools, and compare timestamps (to a
        // timestamp or an RFC3339 string) and durations.
        let equality = || {
            vec![
                Overload::new(Bool, vec![Bool, Bool]),
                Overload::new(Bool, vec![Int, Int]),
                Overload::new(Bool, vec![Double, Double]),
                Overload::new(Bool, vec![Double, Int]),
                Overload::new(Bool, vec![String, String]),
                Overload::new(Bool, vec![Timestamp, Timestamp]),
                Overload::new(Bool, vec![Timestamp, String]),
                Overload::new(Bool, vec![Duration, Duration]),
            ]
        };
        // The ordering operators compare like-typed operands (and a double to an
        // int literal), but not bools; plus the same timestamp/duration overloads.
        let ordering = || {
            vec![
                Overload::new(Bool, vec![Int, Int]),
                Overload::new(Bool, vec![Double, Double]),
                Overload::new(Bool, vec![Double, Int]),
                Overload::new(Bool, vec![String, String]),
                Overload::new(Bool, vec![Timestamp, Timestamp]),
                Overload::new(Bool, vec![Timestamp, String]),
                Overload::new(Bool, vec![Duration, Duration]),
            ]
        };
        // `a : b` tests presence/membership: a substring of a string, a key in a
        // `map<string,string>`, a value in a `list<string>`, or — restricted to
        // the `*` wildcard by the checker — presence of a timestamp field.
        let has = || {
            vec![
                Overload::new(Bool, vec![String, String]),
                Overload::new(Bool, vec![Type::map(String, String), String]),
                Overload::new(Bool, vec![Type::list(String), String]),
                Overload::new(Bool, vec![Timestamp, String]),
            ]
        };
        self.function(function::EQUALS, equality())
            .function(function::NOT_EQUALS, equality())
            .function(function::LESS_THAN, ordering())
            .function(function::LESS_EQUALS, ordering())
            .function(function::GREATER_THAN, ordering())
            .function(function::GREATER_EQUALS, ordering())
            .function(function::HAS, has())
            .function(function::AND, vec![Overload::new(Bool, vec![Bool, Bool])])
            .function(function::OR, vec![Overload::new(Bool, vec![Bool, Bool])])
            .function(function::NOT, vec![Overload::new(Bool, vec![Bool])])
            // `timestamp("...")` / `duration("...")` lift a string literal to a
            // temporal value, so it can be compared via the overloads above.
            .function(
                function::TIMESTAMP,
                vec![Overload::new(Timestamp, vec![String])],
            )
            .function(
                function::DURATION,
                vec![Overload::new(Duration, vec![String])],
            )
    }

    /// Declare a filterable identifier with a type. A repeated name replaces the
    /// earlier declaration.
    ///
    /// The `name` is recorded as a declared **field path**
    /// ([`Declarations::field_paths`]) — it names an addressable field. The enum
    /// *value* names [`enum_ident`](Self::enum_ident) adds go through a private
    /// insert that does **not** record them, so a value name never masquerades as
    /// a field.
    pub fn ident(self, name: &str, ty: Type) -> Self {
        self.value_ident(name, ty).record_field_path(name)
    }

    /// Insert a filterable identifier *without* recording it as a field path —
    /// the enum value-name carrier for [`enum_ident`](Self::enum_ident). A
    /// repeated name replaces the earlier declaration, like [`ident`](Self::ident).
    fn value_ident(mut self, name: &str, ty: Type) -> Self {
        self.idents.insert(name.to_string(), ty);
        self
    }

    /// Record `name` as a declared field path, keeping declaration order and not
    /// duplicating a name already recorded (a re-declaration replaces its type in
    /// [`idents`](Self::idents) but keeps its original position).
    fn record_field_path(mut self, name: &str) -> Self {
        if !self.field_paths.iter().any(|p| p == name) {
            self.field_paths.push(name.to_string());
        }
        self
    }

    /// Declare an enum-typed identifier (the one reflective declaration).
    ///
    /// `name` becomes an [`Enum`](Type::Enum) identifier carrying the
    /// descriptor's full name, and `=` / `!=` gain an overload comparing two
    /// values of that enum. Each of the enum's value names is also declared as
    /// an identifier of the same enum type, so a filter can name a value bare
    /// (`state = ACTIVE`). Declaring the same enum again re-adds these (an
    /// already-declared value name is replaced, like [`ident`](Self::ident)).
    pub fn enum_ident(self, name: &str, descriptor: EnumDescriptor) -> Self {
        let enum_type = Type::Enum(descriptor.full_name().to_string());
        let comparison = vec![Overload::new(
            Type::Bool,
            vec![enum_type.clone(), enum_type.clone()],
        )];
        let mut builder = self
            .ident(name, enum_type.clone())
            .function(function::EQUALS, comparison.clone())
            .function(function::NOT_EQUALS, comparison);
        for value in descriptor.values() {
            // A value name (`ACTIVE`) is a filter literal, not an addressable
            // field, so it is inserted without recording a field path — even
            // though it shares the field's `Enum` type.
            builder = builder.value_ident(value.name(), enum_type.clone());
        }
        builder
    }

    /// Derive filterable identifiers from the message descriptor, one per named
    /// `path`, deriving each identifier's [`Type`] from the descriptor instead of
    /// the caller spelling it out.
    ///
    /// Requires the builder to carry a descriptor (start from
    /// [`Declarations::for_message`]). Each `path` resolves field-by-field,
    /// descending through `.`-separated subfields into nested **singular** message
    /// fields, and the leaf field's kind fixes the [`Type`]:
    ///
    /// - string → [`String`](Type::String); double/float → [`Double`](Type::Double);
    ///   signed ints → [`Int`](Type::Int); unsigned ints → [`Uint`](Type::Uint);
    ///   bool → [`Bool`](Type::Bool)
    /// - `google.protobuf.Timestamp` → [`Timestamp`](Type::Timestamp)
    /// - an enum gets the full [`enum_ident`](Self::enum_ident) treatment (the
    ///   path, every value name, and the `=` / `!=` overloads) — no caller-side
    ///   descriptor extraction
    /// - a map field → [`Type::map`] with key/value types from the entry descriptor
    /// - a repeated field → [`Type::list`] with the element type from the field kind
    ///
    /// An unknown path or an unfilterable leaf kind (e.g. bytes, or a non-Timestamp
    /// singular message) is **not** silently skipped: the first such failure is
    /// recorded and surfaced at [`build`](Self::build) as a descriptive
    /// [`Error`] naming the path. The explicit [`ident`](Self::ident) /
    /// [`enum_ident`](Self::enum_ident) path stays available for anything the
    /// descriptor can't supply.
    pub fn fields<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for path in paths {
            if self.deferred.is_some() {
                break;
            }
            let path = path.as_ref();
            let Some(descriptor) = self.descriptor.clone() else {
                self.deferred = Some(Error::Type(
                    "fields() requires a message descriptor; \
                     start from Declarations::for_message"
                        .to_owned(),
                ));
                break;
            };
            match resolve_path(&descriptor, path) {
                Ok(Resolved::Scalar(ty)) => self = self.ident(path, ty),
                Ok(Resolved::Enum(enum_descriptor)) => {
                    self = self.enum_ident(path, enum_descriptor)
                }
                Err(error) => self.deferred = Some(error),
            }
        }
        self
    }

    /// Declare a function with the given resolvable `overloads`. Declaring the
    /// same name again appends overloads to the existing declaration.
    pub fn function(mut self, name: &str, overloads: Vec<Overload>) -> Self {
        self.functions
            .entry(name.to_string())
            .or_default()
            .extend(overloads);
        self
    }

    /// Finalize the declarations, panicking if a derivation
    /// ([`fields`](Self::fields)) produced an error. A bad declaration set is a
    /// programmer bug (misconfigured field paths) — like `Regex::new` with a
    /// known-good pattern, callers use this on static config where panicking on
    /// misconfiguration is correct. Use [`try_build`](Self::try_build) when the
    /// paths are dynamic and the caller must handle the error.
    #[track_caller]
    pub fn build(self) -> Declarations {
        self.try_build().expect("filter declarations are valid")
    }

    /// Finalize the declarations, returning the first error a derivation
    /// ([`fields`](Self::fields)) deferred.
    pub fn try_build(self) -> Result<Declarations, Error> {
        if let Some(error) = self.deferred {
            return Err(error);
        }
        Ok(Declarations {
            idents: self.idents,
            functions: self.functions,
            field_paths: self.field_paths,
        })
    }
}

/// The outcome of resolving a [`fields`](DeclarationsBuilder::fields) path: a
/// directly-declarable [`Type`], or an enum that needs the full
/// [`enum_ident`](DeclarationsBuilder::enum_ident) treatment.
enum Resolved {
    Scalar(Type),
    Enum(EnumDescriptor),
}

/// Resolves a `.`-separated `path` against `descriptor`, descending through
/// singular message fields, and derives the leaf field's [`Resolved`] type.
fn resolve_path(descriptor: &MessageDescriptor, path: &str) -> Result<Resolved, Error> {
    let unknown = || Error::UnknownField {
        path: path.to_owned(),
        message: descriptor.full_name().to_owned(),
    };
    let mut current = descriptor.clone();
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        let Some(field) = current.get_field_by_name(segment) else {
            return Err(unknown());
        };
        if segments.peek().is_none() {
            return resolve_leaf(descriptor, path, &field);
        }
        // A non-leaf segment must name a singular message to descend into; a
        // scalar, enum, map, or repeated field has no subfields to address.
        match descend(&field) {
            Some(message) => current = message,
            None => return Err(unknown()),
        }
    }
    Err(unknown()) // an empty path resolves to nothing
}

/// The message to descend into through a non-leaf `field`: its own message type,
/// but only when it is singular. `None` for a scalar/enum leaf or a map/repeated
/// field (whose elements have no addressable subfield path here).
fn descend(field: &FieldDescriptor) -> Option<MessageDescriptor> {
    if field.is_list() || field.is_map() {
        return None;
    }
    match field.kind() {
        Kind::Message(message) => Some(message),
        _ => None,
    }
}

/// Derives the [`Resolved`] type of a leaf `field`: a `map<k,v>` / `list<e>` for
/// a map / repeated field, the full enum treatment for a singular enum, otherwise
/// the scalar [`Type`] for the field kind.
fn resolve_leaf(
    descriptor: &MessageDescriptor,
    path: &str,
    field: &FieldDescriptor,
) -> Result<Resolved, Error> {
    let unfilterable = |detail: String| Error::UnfilterableField {
        path: path.to_owned(),
        message: descriptor.full_name().to_owned(),
        detail,
    };
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            return Err(unfilterable("map entry is not a message".to_owned()));
        };
        let key = kind_to_type(&entry.map_entry_key_field().kind()).map_err(&unfilterable)?;
        let value = kind_to_type(&entry.map_entry_value_field().kind()).map_err(&unfilterable)?;
        return Ok(Resolved::Scalar(Type::map(key, value)));
    }
    if field.is_list() {
        let element = kind_to_type(&field.kind()).map_err(&unfilterable)?;
        return Ok(Resolved::Scalar(Type::list(element)));
    }
    // A singular enum gets the full `enum_ident` treatment, not a bare type.
    if let Kind::Enum(enum_descriptor) = field.kind() {
        return Ok(Resolved::Enum(enum_descriptor));
    }
    Ok(Resolved::Scalar(
        kind_to_type(&field.kind()).map_err(&unfilterable)?,
    ))
}

/// Maps a scalar proto field [`Kind`] to a filter [`Type`], for a map key/value
/// or a list element. The unfilterable kinds — bytes, any
/// non-`google.protobuf.Timestamp` message, and an enum (which is filterable
/// only as a *singular* field, where it gets the full
/// [`enum_ident`](DeclarationsBuilder::enum_ident) treatment via
/// [`Resolved::Enum`] before this is reached) — return the kind's descriptive
/// name in `Err` for the caller's [`Error`].
fn kind_to_type(kind: &Kind) -> Result<Type, String> {
    Ok(match kind {
        Kind::Double | Kind::Float => Type::Double,
        Kind::Int32
        | Kind::Int64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Sfixed32
        | Kind::Sfixed64 => Type::Int,
        Kind::Uint32 | Kind::Uint64 | Kind::Fixed32 | Kind::Fixed64 => Type::Uint,
        Kind::Bool => Type::Bool,
        Kind::String => Type::String,
        Kind::Message(message) if message.full_name() == "google.protobuf.Timestamp" => {
            Type::Timestamp
        }
        Kind::Message(message) => return Err(format!("message {}", message.full_name())),
        Kind::Bytes => return Err("bytes".to_owned()),
        // A list/map *of* enum can't carry the per-enum `=`/`!=` overloads that
        // make a bare value name (`state = ACTIVE`) type-check, so it is not
        // filterable; a singular enum is intercepted before this in `resolve_leaf`.
        Kind::Enum(enum_descriptor) => {
            return Err(format!(
                "enum {} (filterable only as a singular field)",
                enum_descriptor.full_name()
            ))
        }
    })
}

/// Parses a filter string into an AST without type-checking.
pub fn parse(filter: &str) -> Result<Expr, Error> {
    parser::parse_filter(filter)
}

/// Parses and type-checks a filter against `declarations`.
///
/// An empty (or whitespace-only) filter is a syntax error: callers treating an
/// empty `filter` field as "no filter" should skip this call entirely.
pub fn check(filter: &str, declarations: &Declarations) -> Result<Filter, Error> {
    let expr = parser::parse_filter(filter)?;
    checker::check(&expr, declarations)?;
    Ok(Filter { expr })
}

/// A request carrying an AIP-160 `filter` string.
pub trait FilterRequest {
    /// The `filter` field of the request.
    fn filter(&self) -> &str;
}

#[cfg_attr(docsrs, doc(cfg(feature = "cel-proto")))]
#[cfg(feature = "cel-proto")]
pub mod cel_proto;

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

#[cfg_attr(docsrs, doc(cfg(feature = "tonic")))]
#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    /// Maps to `INVALID_ARGUMENT` with AIP-193 standard details: an `ErrorInfo`
    /// carrying a machine-readable `reason` + [`domain`](ERROR_DOMAIN) and the
    /// error's dynamic values as `metadata`. A filter error points inside the
    /// filter expression rather than at a request field path, so no `BadRequest`
    /// is attached. See `docs/adr/0007-aip193-error-details.md`.
    fn from(err: Error) -> Self {
        use std::collections::HashMap;
        use tonic_types::{ErrorDetails, StatusExt};

        let message = err.to_string();
        let (reason, metadata): (&str, HashMap<String, String>) = match &err {
            Error::Syntax { position, message } => (
                "FILTER_SYNTAX",
                HashMap::from([
                    ("position".to_owned(), position.to_string()),
                    ("detail".to_owned(), message.clone()),
                ]),
            ),
            Error::Type(detail) => (
                "FILTER_TYPE",
                HashMap::from([("detail".to_owned(), detail.clone())]),
            ),
            Error::UndeclaredIdent(ident) => (
                "FILTER_UNDECLARED_IDENTIFIER",
                HashMap::from([("identifier".to_owned(), ident.clone())]),
            ),
            // The descriptor-derivation errors are raised while *building*
            // declarations (server construction), so they do not normally reach
            // a request; mapped defensively for the exhaustive match.
            Error::UnknownField { path, message } => (
                "FILTER_UNKNOWN_FIELD",
                HashMap::from([
                    ("path".to_owned(), path.clone()),
                    ("message".to_owned(), message.clone()),
                ]),
            ),
            Error::UnfilterableField {
                path,
                message,
                detail,
            } => (
                "FILTER_UNFILTERABLE_FIELD",
                HashMap::from([
                    ("path".to_owned(), path.clone()),
                    ("message".to_owned(), message.clone()),
                    ("detail".to_owned(), detail.clone()),
                ]),
            ),
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
    }
}

/// Tests for the descriptor-driven [`fields`](DeclarationsBuilder::fields) path.
///
/// These exercise the public `fields` → `build` flow against fixture descriptors
/// from the shared pool. They construct the builder with the descriptor set
/// directly (the in-crate equivalent of [`Declarations::for_message`], which is
/// just `M::default().descriptor()` but needs a generated typed message the
/// fixture pool does not provide).
#[cfg(test)]
mod derive_tests {
    use super::*;

    /// The syntax fixture message carrying one field of every proto kind.
    fn syntax_message() -> MessageDescriptor {
        test_fixtures::message_descriptor("einride.example.syntax.v1.Message")
            .expect("the syntax Message is in the fixture pool")
    }

    /// The freight Site fixture — its `create_time` (Timestamp), `lat_lng`
    /// (nested message), `state` (enum), `annotations` (map), and `tags` (list)
    /// cover the shapes the syntax message's flat scalars do not.
    fn site_message() -> MessageDescriptor {
        test_fixtures::message_descriptor("einride.example.freight.v1.Site")
            .expect("the freight Site is in the fixture pool")
    }

    /// Build declarations deriving `paths` from `descriptor`.
    fn derive(descriptor: MessageDescriptor, paths: &[&str]) -> Result<Declarations, Error> {
        DeclarationsBuilder {
            descriptor: Some(descriptor),
            ..DeclarationsBuilder::default()
        }
        .fields(paths.iter().copied())
        .try_build()
    }

    #[test]
    fn map_and_list_constructors_box() {
        assert_eq!(
            Type::map(Type::String, Type::Int),
            Type::Map(Box::new(Type::String), Box::new(Type::Int)),
        );
        assert_eq!(Type::list(Type::String), Type::List(Box::new(Type::String)),);
    }

    #[test]
    fn derives_scalar_kinds_from_the_descriptor() {
        let decls = derive(
            syntax_message(),
            &[
                "double", "float", "int32", "int64", "sint32", "sfixed64", "uint32", "fixed64",
                "bool", "string",
            ],
        )
        .expect("scalar fields derive");
        let ty = |name| decls.ident_type(name).expect("declared");
        assert_eq!(ty("double"), &Type::Double);
        assert_eq!(ty("float"), &Type::Double);
        assert_eq!(ty("int32"), &Type::Int);
        assert_eq!(ty("int64"), &Type::Int);
        assert_eq!(ty("sint32"), &Type::Int);
        assert_eq!(ty("sfixed64"), &Type::Int);
        assert_eq!(ty("uint32"), &Type::Uint);
        assert_eq!(ty("fixed64"), &Type::Uint);
        assert_eq!(ty("bool"), &Type::Bool);
        assert_eq!(ty("string"), &Type::String);
    }

    #[test]
    fn derives_timestamp_and_nested_paths_from_the_site() {
        // The fixture Site's `create_time` (Timestamp) and the nested
        // `lat_lng.latitude` (descend a singular message to a double).
        let decls = derive(site_message(), &["create_time", "lat_lng.latitude"])
            .expect("site fields derive");
        assert_eq!(decls.ident_type("create_time"), Some(&Type::Timestamp));
        assert_eq!(decls.ident_type("lat_lng.latitude"), Some(&Type::Double));
    }

    #[test]
    fn derives_map_and_list_element_types() {
        let decls = derive(
            syntax_message(),
            &["map_string_string", "repeated_int64", "repeated_string"],
        )
        .expect("map and repeated fields derive");
        assert_eq!(
            decls.ident_type("map_string_string"),
            Some(&Type::map(Type::String, Type::String)),
        );
        assert_eq!(
            decls.ident_type("repeated_int64"),
            Some(&Type::list(Type::Int)),
        );
        assert_eq!(
            decls.ident_type("repeated_string"),
            Some(&Type::list(Type::String)),
        );
    }

    #[test]
    fn enum_field_gets_full_enum_treatment() {
        let decls = derive(syntax_message(), &["enum"]).expect("the enum field derives");
        // The field and every value name resolve to the same enum type...
        let enum_ty = Type::Enum("einride.example.syntax.v1.Enum".to_owned());
        assert_eq!(decls.ident_type("enum"), Some(&enum_ty));
        assert_eq!(decls.ident_type("ENUM_ONE"), Some(&enum_ty));
        assert_eq!(decls.ident_type("ENUM_TWO"), Some(&enum_ty));
        // ...and the `=` overload comparing two of them is declared, so a bare
        // value name type-checks against the field with no caller-side extraction.
        let decls = DeclarationsBuilder {
            descriptor: Some(syntax_message()),
            ..DeclarationsBuilder::default()
        }
        .standard_functions()
        .fields(["enum"])
        .build();
        check("enum = ENUM_ONE", &decls).expect("the derived enum overload type-checks");
    }

    #[test]
    fn field_paths_are_the_declared_fields_in_order_excluding_enum_values() {
        // `state` (enum) is a declared field; its value names (`ENUM_ONE`, …) are
        // filter literals carrying the same `Enum` type, and must NOT surface as
        // field paths — that is exactly what a column derivation must not list.
        let decls = derive(syntax_message(), &["string", "int32", "enum"])
            .expect("scalar + enum fields derive");
        let paths: Vec<_> = decls.field_paths().map(|(name, _)| name).collect();
        assert_eq!(paths, vec!["string", "int32", "enum"]);
        // The value names are still declared idents (so a bare value type-checks)...
        let enum_ty = Type::Enum("einride.example.syntax.v1.Enum".to_owned());
        assert_eq!(decls.ident_type("ENUM_ONE"), Some(&enum_ty));
        // ...but they are not field paths.
        assert!(decls.field_paths().all(|(name, _)| name != "ENUM_ONE"));
    }

    #[test]
    fn field_paths_carry_the_declared_types() {
        let decls = derive(syntax_message(), &["string", "int32"]).expect("fields derive");
        let by_name: std::collections::HashMap<_, _> = decls.field_paths().collect();
        assert_eq!(by_name.get("string"), Some(&&Type::String));
        assert_eq!(by_name.get("int32"), Some(&&Type::Int));
    }

    #[test]
    fn redeclaring_a_field_keeps_one_path_at_its_original_position() {
        // A repeated `ident` replaces the type but must not duplicate the path
        // (and keeps declaration order), so a derived column map stays one-per-field.
        let decls = Declarations::builder()
            .ident("a", Type::Int)
            .ident("b", Type::String)
            .ident("a", Type::Double)
            .build();
        let paths: Vec<_> = decls.field_paths().map(|(name, _)| name).collect();
        assert_eq!(paths, vec!["a", "b"]);
        assert_eq!(decls.ident_type("a"), Some(&Type::Double));
    }

    #[test]
    fn unknown_path_fails_build_naming_the_path() {
        let err = derive(syntax_message(), &["not_a_field"]).expect_err("an unknown path fails");
        match err {
            Error::UnknownField { path, message } => {
                assert_eq!(path, "not_a_field");
                assert_eq!(message, "einride.example.syntax.v1.Message");
            }
            other => panic!("expected UnknownField, got {other:?}"),
        }
    }

    #[test]
    fn descending_through_a_scalar_is_an_unknown_path() {
        // `string` is a scalar leaf, so `string.foo` cannot resolve.
        let err = derive(syntax_message(), &["string.foo"]).expect_err("cannot descend a scalar");
        assert!(matches!(err, Error::UnknownField { .. }));
    }

    #[test]
    fn unfilterable_kinds_fail_build() {
        // `repeated_enum` / `map_string_message` confirm an enum or message
        // *inside* a list / map is rejected — only a singular enum is filterable.
        for path in [
            "bytes",
            "repeated_bytes",
            "repeated_enum",
            "message",
            "map_string_message",
        ] {
            let err =
                derive(syntax_message(), &[path]).expect_err("an unfilterable kind fails build");
            match err {
                Error::UnfilterableField {
                    path: errored_path, ..
                } => assert_eq!(errored_path, path),
                other => panic!("expected UnfilterableField for {path:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn first_failure_wins_and_short_circuits() {
        // Two bad paths: build reports the first, not the second.
        let err = derive(syntax_message(), &["ghost", "bytes"]).expect_err("first failure wins");
        match err {
            Error::UnknownField { path, .. } => assert_eq!(path, "ghost"),
            other => panic!("expected the first failure (UnknownField ghost), got {other:?}"),
        }
    }

    #[test]
    fn fields_without_a_descriptor_is_a_build_error() {
        let err = Declarations::builder()
            .fields(["anything"])
            .try_build()
            .expect_err("fields() needs a descriptor");
        assert!(matches!(err, Error::Type(_)));
    }
}

#[cfg(all(test, feature = "tonic"))]
mod tonic_tests {
    use super::*;
    use tonic_types::StatusExt as _;

    #[test]
    fn undeclared_identifier_maps_to_invalid_argument_with_metadata() {
        let status: tonic::Status = Error::UndeclaredIdent("ghost".to_owned()).into();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        let info = status
            .get_details_error_info()
            .expect("an ErrorInfo is always attached (AIP-193)");
        assert_eq!(info.reason, "FILTER_UNDECLARED_IDENTIFIER");
        assert_eq!(info.domain, ERROR_DOMAIN);
        assert_eq!(
            info.metadata.get("identifier").map(String::as_str),
            Some("ghost"),
        );

        // A filter error points inside the expression, not at a request field.
        assert!(status.get_details_bad_request().is_none());
    }
}
