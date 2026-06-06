//! AIP-160 filtering: parse and type-check filter expressions into a native AST.
//!
//! The AST is a native Rust enum (not the CEL proto) — it's filtering's primary
//! product, so it's built to be walked. Optional CEL-proto interop lives behind
//! the `cel-proto` feature. See `docs/adr/0003-native-filter-ast.md`.
//!
//! Declarations are explicit (an allowlist of filterable identifiers); the parse
//! and check core is reflection-free, with `enum_ident` the one reflective hook.
//!
//! See <https://google.aip.dev/160>.

use prost_reflect::EnumDescriptor;

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
    /// Function or operator call (e.g. `_==_`, `:`, `AND`).
    Call { function: String, args: Vec<Expr> },
}

/// A parsed and type-checked filter — filtering's public product.
#[derive(Debug, Clone)]
pub struct Filter {
    /// The type-checked expression tree.
    pub expr: Expr,
}

/// The typed schema a [`Filter`] is checked against: an allowlist of filterable
/// identifiers, plus declared functions and enums.
#[derive(Debug, Clone, Default)]
pub struct Declarations {}

impl Declarations {
    /// Start building a set of declarations.
    pub fn builder() -> DeclarationsBuilder {
        DeclarationsBuilder::default()
    }
}

/// Builder for [`Declarations`] (replaces `aip-go`'s functional options).
#[derive(Debug, Default)]
pub struct DeclarationsBuilder {}

impl DeclarationsBuilder {
    /// Declare the standard AIP-160 function and operator set.
    pub fn standard_functions(self) -> Self {
        todo!()
    }

    /// Declare a filterable identifier with a type.
    pub fn ident(self, _name: &str, _ty: Type) -> Self {
        todo!()
    }

    /// Declare an enum-typed identifier (the one reflective declaration).
    pub fn enum_ident(self, _name: &str, _descriptor: EnumDescriptor) -> Self {
        todo!()
    }

    /// Declare a custom function (overloads added via the returned builder).
    pub fn function(self, _name: &str) -> Self {
        todo!()
    }

    /// Finalize the declarations.
    pub fn build(self) -> Result<Declarations, Error> {
        todo!()
    }
}

/// Parses a filter string into an AST without type-checking.
pub fn parse(_filter: &str) -> Result<Expr, Error> {
    todo!("lex + parse the AIP-160 grammar")
}

/// Parses and type-checks a filter against `declarations`.
pub fn check(_filter: &str, _declarations: &Declarations) -> Result<Filter, Error> {
    todo!("parse, then resolve idents/overloads and assign types")
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

#[cfg(feature = "tonic")]
impl From<Error> for tonic::Status {
    fn from(err: Error) -> Self {
        tonic::Status::invalid_argument(err.to_string())
    }
}
