//! Parser for the AIP-160 comparison slice.
//!
//! Builds the native [`Expr`] AST from a [restriction](https://google.aip.dev/160):
//!
//! ```text
//! restriction : comparable [comparator arg]
//! comparable  : member | number
//! member      : value {DOT field}
//! arg         : comparable
//! ```
//!
//! Logical composition (`AND`/`OR`/`NOT`), parentheses, functions, the has
//! operator, and macros are later slices, so only the comparison operators
//! `=`, `!=`, `<`, `<=`, `>`, `>=` are accepted. Whitespace is purely a
//! separator here, so it is stripped before parsing.

use crate::token::{self, Token, TokenType};
use crate::{Constant, Error, Expr};

/// Parse `filter` into its native [`Expr`] AST (no type-checking).
pub(crate) fn parse_filter(filter: &str) -> Result<Expr, Error> {
    let tokens: Vec<Token> = crate::lexer::tokenize(filter)?
        .into_iter()
        .filter(|t| t.ty != TokenType::Whitespace)
        .collect();
    if tokens.is_empty() {
        return Err(Error::Syntax {
            position: 0,
            message: "empty filter".to_string(),
        });
    }
    let mut parser = Parser {
        input: filter,
        tokens,
        pos: 0,
    };
    let expr = parser.restriction()?;
    if let Some(token) = parser.peek() {
        return Err(Error::Syntax {
            position: token.offset,
            message: format!("unexpected trailing token `{}`", token.value),
        });
    }
    Ok(expr)
}

struct Parser<'a> {
    input: &'a str,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_type(&self) -> Option<TokenType> {
        self.peek().map(|t| t.ty)
    }

    /// Consume and return (a clone of) the next token.
    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    /// Byte offset to attribute an end-of-input error to.
    fn eof_offset(&self) -> usize {
        self.input.len()
    }

    // restriction : comparable [comparator arg]
    fn restriction(&mut self) -> Result<Expr, Error> {
        let lhs = self.comparable()?;
        let Some(function) = self.peek_type().and_then(TokenType::comparison_function) else {
            return Ok(lhs);
        };
        self.bump(); // the comparator
        let rhs = self.comparable()?;
        Ok(Expr::Call {
            function: function.to_string(),
            args: vec![lhs, rhs],
        })
    }

    // comparable : member | number
    fn comparable(&mut self) -> Result<Expr, Error> {
        if let Some(number) = self.try_number() {
            return Ok(number);
        }
        self.member()
    }

    /// Attempt to parse a number, restoring the cursor and yielding `None` if the
    /// input does not start one (so the caller falls back to a member).
    fn try_number(&mut self) -> Option<Expr> {
        let start = self.pos;
        match self.number() {
            Ok(expr) => Some(expr),
            Err(_) => {
                self.pos = start;
                None
            }
        }
    }

    // number : float | int
    fn number(&mut self) -> Result<Expr, Error> {
        let start = self.pos;
        if let Ok(float) = self.float() {
            return Ok(float);
        }
        self.pos = start;
        self.int()
    }

    // float : MINUS? (NUMBER DOT NUMBER* | DOT NUMBER)
    fn float(&mut self) -> Result<Expr, Error> {
        let offset = self.peek().map_or_else(|| self.eof_offset(), |t| t.offset);
        let mut literal = String::new();
        if self.peek_type() == Some(TokenType::Minus) {
            literal.push('-');
            self.bump();
        }
        let has_int = self.peek_type() == Some(TokenType::Number);
        if has_int {
            literal.push_str(&self.bump().expect("peeked a number").value);
        }
        if self.peek_type() != Some(TokenType::Dot) {
            return Err(self.error_at(offset, "expected `.` in float"));
        }
        literal.push('.');
        self.bump();
        let has_fraction = self.peek_type() == Some(TokenType::Number);
        if has_fraction {
            literal.push_str(&self.bump().expect("peeked a number").value);
        }
        if !has_int && !has_fraction {
            return Err(self.error_at(offset, "expected digits in float"));
        }
        let value: f64 = literal
            .parse()
            .map_err(|_| self.error_at(offset, format!("invalid float `{literal}`")))?;
        Ok(Expr::Const(Constant::Double(value)))
    }

    // int : MINUS? (NUMBER | HEX)
    fn int(&mut self) -> Result<Expr, Error> {
        let negative = self.peek_type() == Some(TokenType::Minus);
        if negative {
            self.bump();
        }
        let token = match self.peek_type() {
            Some(TokenType::Number | TokenType::HexNumber) => self.bump().expect("peeked a number"),
            _ => return Err(self.unexpected("expected an integer")),
        };
        let magnitude: i64 = if token.ty == TokenType::HexNumber {
            let hex = token
                .value
                .strip_prefix("0x")
                .or_else(|| token.value.strip_prefix("0X"))
                .unwrap_or(&token.value);
            i64::from_str_radix(hex, 16)
        } else {
            token.value.parse()
        }
        .map_err(|_| self.error_at(token.offset, format!("invalid integer `{}`", token.value)))?;
        let value = if negative { -magnitude } else { magnitude };
        Ok(Expr::Const(Constant::Int(value)))
    }

    // member : value {DOT field}
    fn member(&mut self) -> Result<Expr, Error> {
        let base = self.expect(|ty| ty.is_value(), "a value")?;
        if self.peek_type() != Some(TokenType::Dot) {
            // A bare string is a literal; a bare text token is an identifier.
            return if base.ty == TokenType::String {
                Ok(Expr::Const(Constant::String(self.unquote(&base)?)))
            } else {
                Ok(Expr::Ident(base.value))
            };
        }
        // A dotted member's base is always an identifier; each `.field` selects.
        let mut expr = Expr::Ident(self.unquote(&base)?);
        while self.peek_type() == Some(TokenType::Dot) {
            self.bump(); // the dot
            let field = self.expect(|ty| ty.is_field(), "a field")?;
            expr = Expr::Select {
                operand: Box::new(expr),
                field: self.unquote(&field)?,
            };
        }
        Ok(expr)
    }

    /// The logical text of a value/field token: a string token is unescaped, any
    /// other token is taken verbatim.
    fn unquote(&self, token: &Token) -> Result<String, Error> {
        if token.ty == TokenType::String {
            token::unescape(&token.value).map_err(|message| Error::Syntax {
                position: token.offset,
                message,
            })
        } else {
            Ok(token.value.clone())
        }
    }

    /// Consume the next token if it satisfies `pred`, else a syntax error.
    fn expect(&mut self, pred: impl Fn(TokenType) -> bool, what: &str) -> Result<Token, Error> {
        match self.peek() {
            Some(token) if pred(token.ty) => Ok(self.bump().expect("peeked a token")),
            Some(token) => Err(Error::Syntax {
                position: token.offset,
                message: format!("unexpected token `{}`", token.value),
            }),
            None => Err(Error::Syntax {
                position: self.eof_offset(),
                message: format!("expected {what}, found end of input"),
            }),
        }
    }

    fn unexpected(&self, message: &str) -> Error {
        match self.peek() {
            Some(token) => Error::Syntax {
                position: token.offset,
                message: format!("unexpected token `{}`", token.value),
            },
            None => Error::Syntax {
                position: self.eof_offset(),
                message: message.to_string(),
            },
        }
    }

    fn error_at(&self, position: usize, message: impl Into<String>) -> Error {
        Error::Syntax {
            position,
            message: message.into(),
        }
    }
}
