//! Parser for the AIP-160 filter grammar.
//!
//! Builds the native [`Expr`] AST for the full grammar
//! ([EBNF](https://google.aip.dev/assets/misc/ebnf-filtering.txt)):
//!
//! ```text
//! expression  : sequence {WS AND WS sequence}
//! sequence    : factor {WS factor}
//! factor      : term {WS OR WS term}
//! term        : [(NOT WS | MINUS)] simple
//! simple      : restriction | composite
//! composite   : LPAREN expression RPAREN
//! restriction : comparable [comparator arg]
//! comparable  : function | number | member
//! function    : name {DOT name} LPAREN [arg {COMMA arg}] RPAREN
//! member      : value {DOT field}
//! arg         : comparable | composite
//! ```
//!
//! Logical composition lowers to [`Expr::Call`]s: `AND` for explicit
//! conjunction, `FUZZY` for the implicit AND between space-separated factors,
//! `OR` for disjunction, and `NOT` for negation. The has operator `:` is a
//! comparator like the others, except a bare identifier on its right (`m:foo`)
//! is read as the *string* `"foo"` rather than an identifier reference.
//! Whitespace is significant (it separates factors), so — unlike the comparison
//! slice — the lexer's whitespace tokens are kept and consumed explicitly.

use crate::token::{self, Token, TokenType};
use crate::{function, Constant, Error, Expr};

/// Parse `filter` into its native [`Expr`] AST (no type-checking).
pub(crate) fn parse_filter(filter: &str) -> Result<Expr, Error> {
    let mut tokens = crate::lexer::tokenize(filter)?;
    // An empty or whitespace-only filter is not a valid expression.
    if tokens.iter().all(|t| t.ty == TokenType::Whitespace) {
        return Err(Error::Syntax {
            position: 0,
            message: "empty filter".to_string(),
        });
    }
    // Drop trailing whitespace so the sequence loop doesn't try to read a factor
    // past the last one. Offsets (and `eof_offset`) stay relative to `filter`, so
    // error positions are unaffected; leading whitespace is eaten while parsing.
    while tokens.last().is_some_and(|t| t.ty == TokenType::Whitespace) {
        tokens.pop();
    }
    let mut parser = Parser {
        input: filter,
        tokens,
        pos: 0,
    };
    let expr = parser.expression()?;
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
    // ----- token cursor -----

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

    /// Consume the exact `types` in order, returning `true` on success. On a
    /// mismatch the cursor is restored and `false` is returned (so `eat` doubles
    /// as an optional consume when its result is ignored).
    fn eat(&mut self, types: &[TokenType]) -> bool {
        let start = self.pos;
        for &ty in types {
            match self.tokens.get(self.pos) {
                Some(token) if token.ty == ty => self.pos += 1,
                _ => {
                    self.pos = start;
                    return false;
                }
            }
        }
        true
    }

    /// True if the upcoming tokens match `types` exactly (no consumption).
    fn sniff(&self, types: &[TokenType]) -> bool {
        types
            .iter()
            .enumerate()
            .all(|(i, &ty)| self.tokens.get(self.pos + i).is_some_and(|t| t.ty == ty))
    }

    /// True if the upcoming tokens satisfy `preds` in order (no consumption).
    fn sniff_preds(&self, preds: &[fn(TokenType) -> bool]) -> bool {
        preds
            .iter()
            .enumerate()
            .all(|(i, pred)| self.tokens.get(self.pos + i).is_some_and(|t| pred(t.ty)))
    }

    // ----- grammar -----

    // expression : sequence {WS AND WS sequence}
    fn expression(&mut self) -> Result<Expr, Error> {
        let mut sequences = Vec::new();
        loop {
            self.eat(&[TokenType::Whitespace]);
            sequences.push(self.sequence()?);
            if !self.eat(&[TokenType::Whitespace, TokenType::And, TokenType::Whitespace]) {
                break;
            }
        }
        Ok(fold_left(sequences, function::AND))
    }

    // sequence : factor {WS factor}  (the implicit AND, lowered to FUZZY)
    fn sequence(&mut self) -> Result<Expr, Error> {
        let mut factors = Vec::new();
        loop {
            factors.push(self.factor()?);
            // A following `WS AND` belongs to the enclosing expression.
            if self.sniff(&[TokenType::Whitespace, TokenType::And]) {
                break;
            }
            if !self.eat(&[TokenType::Whitespace]) {
                break;
            }
        }
        Ok(fold_left(factors, function::FUZZY))
    }

    // factor : term {WS OR WS term}
    fn factor(&mut self) -> Result<Expr, Error> {
        let mut terms = Vec::new();
        loop {
            terms.push(self.term()?);
            if !self.eat(&[TokenType::Whitespace, TokenType::Or, TokenType::Whitespace]) {
                break;
            }
        }
        Ok(fold_left(terms, function::OR))
    }

    // term : [(NOT WS | MINUS)] simple
    fn term(&mut self) -> Result<Expr, Error> {
        let not = self.eat(&[TokenType::Not, TokenType::Whitespace]);
        let minus = !not && self.eat(&[TokenType::Minus]);
        let simple = self.simple()?;
        if minus {
            // `-` on a numeric literal negates it rather than wrapping in NOT.
            match simple {
                Expr::Const(Constant::Int(v)) => return Ok(Expr::Const(Constant::Int(-v))),
                Expr::Const(Constant::Double(v)) => return Ok(Expr::Const(Constant::Double(-v))),
                _ => {}
            }
        }
        if not || minus {
            Ok(Expr::Call {
                function: function::NOT.to_string(),
                args: vec![simple],
            })
        } else {
            Ok(simple)
        }
    }

    // simple : restriction | composite
    fn simple(&mut self) -> Result<Expr, Error> {
        if self.sniff(&[TokenType::LeftParen]) {
            self.composite()
        } else {
            self.restriction()
        }
    }

    // composite : LPAREN expression RPAREN
    fn composite(&mut self) -> Result<Expr, Error> {
        self.expect(|ty| ty == TokenType::LeftParen, "`(`")?;
        self.eat(&[TokenType::Whitespace]);
        let expr = self.expression()?;
        self.eat(&[TokenType::Whitespace]);
        self.expect(|ty| ty == TokenType::RightParen, "`)`")?;
        Ok(expr)
    }

    // restriction : comparable [comparator arg]
    fn restriction(&mut self) -> Result<Expr, Error> {
        let comparable = self.comparable()?;
        if !self.sniff_preds(&[TokenType::is_comparator])
            && !self.sniff_preds(&[is_whitespace, TokenType::is_comparator])
        {
            return Ok(comparable);
        }
        self.eat(&[TokenType::Whitespace]);
        let comparator = self.expect(TokenType::is_comparator, "a comparator")?;
        self.eat(&[TokenType::Whitespace]);
        let mut arg = self.arg()?;
        // `m:foo` tests whether `m` has the key/value `foo`, so a bare identifier
        // on the right of `:` is its string value, not an identifier reference.
        if comparator.ty == TokenType::Has {
            if let Expr::Ident(name) = arg {
                arg = Expr::Const(Constant::String(name));
            }
        }
        let function = comparator
            .ty
            .comparison_function()
            .expect("a comparator maps to a function")
            .to_string();
        Ok(Expr::Call {
            function,
            args: vec![comparable, arg],
        })
    }

    // comparable : function | number | member
    fn comparable(&mut self) -> Result<Expr, Error> {
        if let Some(function) = self.try_parse(Self::function) {
            return Ok(function);
        }
        if let Some(number) = self.try_parse(Self::number) {
            return Ok(number);
        }
        self.member()
    }

    // arg : comparable | composite
    fn arg(&mut self) -> Result<Expr, Error> {
        if self.sniff(&[TokenType::LeftParen]) {
            self.composite()
        } else {
            self.comparable()
        }
    }

    // function : name {DOT name} LPAREN [arg {COMMA arg}] RPAREN ; name : TEXT | keyword
    fn function(&mut self) -> Result<Expr, Error> {
        let mut name = String::new();
        loop {
            let token = self.expect(TokenType::is_name, "a function name")?;
            name.push_str(&token.value);
            if !self.eat(&[TokenType::Dot]) {
                break;
            }
            name.push('.');
        }
        // A function requires a `(`; without it this is not a function and the
        // caller backtracks to a number or member.
        if !self.eat(&[TokenType::LeftParen]) {
            return Err(self.unexpected("expected `(`"));
        }
        self.eat(&[TokenType::Whitespace]);
        let mut args = Vec::new();
        while !self.sniff(&[TokenType::RightParen]) {
            args.push(self.arg()?);
            self.eat(&[TokenType::Whitespace]);
            if !self.eat(&[TokenType::Comma]) {
                break;
            }
            self.eat(&[TokenType::Whitespace]);
        }
        self.eat(&[TokenType::Whitespace]);
        if !self.eat(&[TokenType::RightParen]) {
            return Err(self.unexpected("expected `)`"));
        }
        Ok(Expr::Call {
            function: name,
            args,
        })
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
        let base = self.expect(TokenType::is_value, "a value")?;
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
            let field = self.expect(TokenType::is_field, "a field")?;
            expr = Expr::Select {
                operand: Box::new(expr),
                field: self.unquote(&field)?,
            };
        }
        Ok(expr)
    }

    // ----- helpers -----

    /// Run `parse`, restoring the cursor and yielding `None` on failure (so the
    /// caller can fall through to the next alternative).
    fn try_parse(&mut self, parse: fn(&mut Self) -> Result<Expr, Error>) -> Option<Expr> {
        let start = self.pos;
        match parse(self) {
            Ok(expr) => Some(expr),
            Err(_) => {
                self.pos = start;
                None
            }
        }
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

/// Left-associate `exprs` under `function`, returning a lone expression as-is.
fn fold_left(exprs: Vec<Expr>, function: &str) -> Expr {
    let mut iter = exprs.into_iter();
    let mut acc = iter.next().expect("at least one expression");
    for expr in iter {
        acc = Expr::Call {
            function: function.to_string(),
            args: vec![acc, expr],
        };
    }
    acc
}

fn is_whitespace(ty: TokenType) -> bool {
    ty == TokenType::Whitespace
}
