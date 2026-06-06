//! Lexer for the AIP-160 filter grammar.
//!
//! [`tokenize`] turns a filter string into a flat [`Vec<Token>`], each carrying
//! its byte offset so syntax errors can point at the offending character. The
//! input is a `&str` (guaranteed valid UTF-8), so — unlike `aip-go` — there is
//! no invalid-UTF-8 error to surface.

use crate::token::{Token, TokenType};
use crate::Error;

/// Tokenize `filter` into its [`Token`] stream (whitespace tokens included).
///
/// The only lexical error is an unterminated string, reported with the byte
/// offset of the opening quote.
pub(crate) fn tokenize(filter: &str) -> Result<Vec<Token>, Error> {
    let mut lexer = Lexer {
        input: filter,
        pos: 0,
    };
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token()? {
        tokens.push(token);
    }
    Ok(tokens)
}

struct Lexer<'a> {
    input: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    /// The unconsumed remainder of the input.
    fn rest(&self) -> &str {
        &self.input[self.pos..]
    }

    /// The next character without consuming it.
    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Consume and return the next character.
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Consume characters while `pred` holds.
    fn bump_while(&mut self, pred: impl Fn(char) -> bool) {
        while self.peek().is_some_and(&pred) {
            self.bump();
        }
    }

    /// Lex the next token, or `None` at end of input.
    fn next_token(&mut self) -> Result<Option<Token>, Error> {
        let start = self.pos;
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let ty = match c {
            '(' => self.single(TokenType::LeftParen),
            ')' => self.single(TokenType::RightParen),
            '-' => self.single(TokenType::Minus),
            '.' => self.single(TokenType::Dot),
            '=' => self.single(TokenType::Equals),
            ':' => self.single(TokenType::Has),
            ',' => self.single(TokenType::Comma),
            // Two-character operators: `<`, `>`, `!` optionally followed by `=`.
            '<' => self.maybe_equals(TokenType::LessThan, TokenType::LessEquals),
            '>' => self.maybe_equals(TokenType::GreaterThan, TokenType::GreaterEquals),
            '!' => self.maybe_equals(TokenType::Exclaim, TokenType::NotEquals),
            '\'' | '"' => self.string(start, c)?,
            '0'..='9' => self.number(),
            c if c.is_whitespace() => {
                self.bump_while(char::is_whitespace);
                TokenType::Whitespace
            }
            _ => self.text(),
        };
        Ok(Some(Token {
            ty,
            offset: start,
            value: self.input[start..self.pos].to_string(),
        }))
    }

    /// Consume one character and emit `ty`.
    fn single(&mut self, ty: TokenType) -> TokenType {
        self.bump();
        ty
    }

    /// Consume `<`/`>`/`!`; emit `two` if `=` follows, else `one`.
    fn maybe_equals(&mut self, one: TokenType, two: TokenType) -> TokenType {
        self.bump();
        if self.peek() == Some('=') {
            self.bump();
            two
        } else {
            one
        }
    }

    /// Lex a quoted string, consuming through the matching closing quote.
    fn string(&mut self, start: usize, quote: char) -> Result<TokenType, Error> {
        self.bump(); // opening quote
        let mut escaped = false;
        loop {
            let Some(c) = self.bump() else {
                return Err(Error::Syntax {
                    position: start,
                    message: "unterminated string".to_string(),
                });
            };
            if c == '\\' && !escaped {
                escaped = true;
                continue;
            }
            if c == quote && !escaped {
                return Ok(TokenType::String);
            }
            escaped = false;
        }
    }

    /// Lex a number: a hex integer (`0x…`) or a run of decimal digits. A `.` is a
    /// separate token, so the parser assembles floats from `NUMBER . NUMBER`.
    fn number(&mut self) -> TokenType {
        self.bump(); // first digit
        if self.peek() == Some('x') {
            self.bump();
            self.bump_while(|c| c.is_ascii_hexdigit());
            TokenType::HexNumber
        } else {
            self.bump_while(|c| c.is_ascii_digit());
            TokenType::Number
        }
    }

    /// Lex a text run, classifying it as a keyword (`NOT`/`AND`/`OR`) or `TEXT`.
    fn text(&mut self) -> TokenType {
        let start = self.pos;
        self.bump_while(is_text);
        TokenType::keyword(&self.input[start..self.pos]).unwrap_or(TokenType::Text)
    }
}

/// True for characters that may appear in an unquoted text token: anything that
/// is not an operator character and not whitespace.
fn is_text(c: char) -> bool {
    !matches!(c, '(' | ')' | '-' | '.' | '=' | ':' | '<' | '>' | '!' | ',') && !c.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert the `(type, offset, value)` view of every token of `filter`.
    fn assert_lex(filter: &str, expected: &[(TokenType, usize, &str)]) {
        let tokens = tokenize(filter).expect("filter lexes");
        let actual: Vec<(TokenType, usize, &str)> = tokens
            .iter()
            .map(|t| (t.ty, t.offset, t.value.as_str()))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn lexes_text_and_whitespace() {
        use TokenType::*;
        assert_lex(
            "New York Giants",
            &[
                (Text, 0, "New"),
                (Whitespace, 3, " "),
                (Text, 4, "York"),
                (Whitespace, 8, " "),
                (Text, 9, "Giants"),
            ],
        );
    }

    #[test]
    fn lexes_keywords_and_parens() {
        use TokenType::*;
        assert_lex(
            "(a OR b) AND NOT c",
            &[
                (LeftParen, 0, "("),
                (Text, 1, "a"),
                (Whitespace, 2, " "),
                (Or, 3, "OR"),
                (Whitespace, 5, " "),
                (Text, 6, "b"),
                (RightParen, 7, ")"),
                (Whitespace, 8, " "),
                (And, 9, "AND"),
                (Whitespace, 12, " "),
                (Not, 13, "NOT"),
                (Whitespace, 16, " "),
                (Text, 17, "c"),
            ],
        );
    }

    #[test]
    fn lexes_comparison_operators() {
        use TokenType::*;
        assert_lex(
            "a < 10 >= != = > ! :",
            &[
                (Text, 0, "a"),
                (Whitespace, 1, " "),
                (LessThan, 2, "<"),
                (Whitespace, 3, " "),
                (Number, 4, "10"),
                (Whitespace, 6, " "),
                (GreaterEquals, 7, ">="),
                (Whitespace, 9, " "),
                (NotEquals, 10, "!="),
                (Whitespace, 12, " "),
                (Equals, 13, "="),
                (Whitespace, 14, " "),
                (GreaterThan, 15, ">"),
                (Whitespace, 16, " "),
                (Exclaim, 17, "!"),
                (Whitespace, 18, " "),
                (Has, 19, ":"),
            ],
        );
    }

    #[test]
    fn lexes_numbers_dots_and_minus() {
        use TokenType::*;
        // A float is `NUMBER DOT NUMBER`; the leading minus is its own token.
        assert_lex(
            "-2.5",
            &[
                (Minus, 0, "-"),
                (Number, 1, "2"),
                (Dot, 2, "."),
                (Number, 3, "5"),
            ],
        );
        assert_lex("0x2A", &[(HexNumber, 0, "0x2A")]);
        assert_lex(
            "a.b.1",
            &[
                (Text, 0, "a"),
                (Dot, 1, "."),
                (Text, 2, "b"),
                (Dot, 3, "."),
                (Number, 4, "1"),
            ],
        );
    }

    #[test]
    fn lexes_strings_with_escapes() {
        use TokenType::*;
        // String token value keeps the quotes; escaped quotes don't terminate.
        assert_lex(r#""a\"b""#, &[(String, 0, r#""a\"b""#)]);
        assert_lex("'hi'", &[(String, 0, "'hi'")]);
    }

    #[test]
    fn unterminated_string_carries_position() {
        let err = tokenize(r#"a = "foo"#).expect_err("unterminated string is an error");
        match err {
            Error::Syntax { position, message } => {
                assert_eq!(position, 4, "points at the opening quote");
                assert!(message.contains("unterminated"), "message: {message}");
            }
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        assert!(tokenize("").expect("empty lexes").is_empty());
    }
}
