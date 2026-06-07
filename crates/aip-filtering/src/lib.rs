//! AIP-160 filtering: parse and type-check filter expressions into a native AST.
//!
//! The AST is a native Rust enum (not the CEL proto) — it's filtering's primary
//! product, so it's built to be walked. Optional CEL-proto interop lives behind
//! the `cel-proto` feature. See `docs/adr/0003-native-filter-ast.md`.
//!
//! Declarations are explicit (an allowlist of filterable identifiers and
//! functions); the parse and check core is reflection-free, with `enum_ident`
//! the one reflective hook.
//!
//! See <https://google.aip.dev/160>.

use std::collections::HashMap;

use prost_reflect::EnumDescriptor;

mod checker;
mod lexer;
mod macros;
mod parser;
mod token;

pub use macros::{apply_macros, Cursor};

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
}

impl Declarations {
    /// Start building a set of declarations.
    pub fn builder() -> DeclarationsBuilder {
        DeclarationsBuilder::default()
    }

    /// Look up a declared identifier's type by name.
    pub(crate) fn lookup_ident(&self, name: &str) -> Option<&Type> {
        self.idents.get(name)
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
}

impl DeclarationsBuilder {
    /// Declare the standard AIP-160 comparison and logical operators with their
    /// standard overloads: `=` / `!=` (bool, int, double, double/int, string),
    /// the ordering operators `<` / `<=` / `>` / `>=` (int, double, double/int,
    /// string), and `AND` / `OR` / `NOT` over bools.
    ///
    /// The `:` (has) operator and the timestamp/duration and enum overloads land
    /// with their own slices.
    pub fn standard_functions(self) -> Self {
        use Type::{Bool, Double, Int, String};
        // `=` / `!=` additionally accept two bools.
        let equality = || {
            vec![
                Overload::new(Bool, vec![Bool, Bool]),
                Overload::new(Bool, vec![Int, Int]),
                Overload::new(Bool, vec![Double, Double]),
                Overload::new(Bool, vec![Double, Int]),
                Overload::new(Bool, vec![String, String]),
            ]
        };
        // The ordering operators compare like-typed operands (and a double to an
        // int literal), but not bools.
        let ordering = || {
            vec![
                Overload::new(Bool, vec![Int, Int]),
                Overload::new(Bool, vec![Double, Double]),
                Overload::new(Bool, vec![Double, Int]),
                Overload::new(Bool, vec![String, String]),
            ]
        };
        self.function(function::EQUALS, equality())
            .function(function::NOT_EQUALS, equality())
            .function(function::LESS_THAN, ordering())
            .function(function::LESS_EQUALS, ordering())
            .function(function::GREATER_THAN, ordering())
            .function(function::GREATER_EQUALS, ordering())
            .function(function::AND, vec![Overload::new(Bool, vec![Bool, Bool])])
            .function(function::OR, vec![Overload::new(Bool, vec![Bool, Bool])])
            .function(function::NOT, vec![Overload::new(Bool, vec![Bool])])
    }

    /// Declare a filterable identifier with a type. A repeated name replaces the
    /// earlier declaration.
    pub fn ident(mut self, name: &str, ty: Type) -> Self {
        self.idents.insert(name.to_string(), ty);
        self
    }

    /// Declare an enum-typed identifier (the one reflective declaration).
    pub fn enum_ident(self, _name: &str, _descriptor: EnumDescriptor) -> Self {
        todo!("enum identifiers land with the enum/well-known-type slice")
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

    /// Finalize the declarations.
    pub fn build(self) -> Result<Declarations, Error> {
        Ok(Declarations {
            idents: self.idents,
            functions: self.functions,
        })
    }
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

#[cfg(feature = "cel-proto")]
pub mod cel_proto {
    //! Conversion between the native AST and `google.api.expr.v1alpha1` CEL
    //! protos. The generated CEL types and `From`/`Into` impls live here.
}

/// The AIP-193 `ErrorInfo.domain` for every error this crate maps. Reason codes
/// are unique within this domain. See `docs/adr/0007-aip193-error-details.md`.
#[cfg(feature = "tonic")]
const ERROR_DOMAIN: &str = "aip-rs";

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
        };
        let mut details = ErrorDetails::new();
        details.set_error_info(reason, ERROR_DOMAIN, metadata);
        tonic::Status::with_error_details(tonic::Code::InvalidArgument, message, details)
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
