//! Tokens for the AIP-160 filter grammar.
//!
//! Internal to the crate: the [`crate::lexer`] produces these and the
//! [`crate::parser`] consumes them. The token set covers the whole AIP-160
//! grammar (see <https://google.aip.dev/assets/misc/ebnf-filtering.txt>) so the
//! lexer is a stable foundation; the comparison-slice parser only consumes a
//! subset (members, numbers, strings, and the comparison operators).

/// The kind of a lexed token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenType {
    /// A run of whitespace (a separator the parser eats).
    Whitespace,
    /// An unquoted identifier or value, e.g. `display_name`.
    Text,
    /// A quoted string literal, e.g. `"acme"` — value includes the quotes.
    String,
    /// The `NOT` keyword.
    Not,
    /// The `AND` keyword.
    And,
    /// The `OR` keyword.
    Or,
    /// A decimal integer, e.g. `42`.
    Number,
    /// A hexadecimal integer, e.g. `0x2A`.
    HexNumber,
    /// `(`
    LeftParen,
    /// `)`
    RightParen,
    /// `-`
    Minus,
    /// `.`
    Dot,
    /// `=`
    Equals,
    /// `:` — the has operator.
    Has,
    /// `<`
    LessThan,
    /// `>`
    GreaterThan,
    /// `!`
    Exclaim,
    /// `,`
    Comma,
    /// `<=`
    LessEquals,
    /// `>=`
    GreaterEquals,
    /// `!=`
    NotEquals,
}

impl TokenType {
    /// The AST function name a comparison operator maps to (`=`, `!=`, `:`, …),
    /// or `None` if this token is not a comparison operator. The `:` (has)
    /// operator is a comparator in the full grammar and maps to `:`.
    pub(crate) fn comparison_function(self) -> Option<&'static str> {
        Some(match self {
            Self::Equals => "=",
            Self::NotEquals => "!=",
            Self::LessThan => "<",
            Self::LessEquals => "<=",
            Self::GreaterThan => ">",
            Self::GreaterEquals => ">=",
            Self::Has => ":",
            _ => return None,
        })
    }

    /// True if the token is a comparator the parser accepts in a restriction.
    pub(crate) fn is_comparator(self) -> bool {
        self.comparison_function().is_some()
    }

    /// True if the token can begin a member: a `TEXT` or a `STRING`.
    pub(crate) fn is_value(self) -> bool {
        matches!(self, Self::Text | Self::String)
    }

    /// True if the token can name a function: a `TEXT` or a keyword (a quoted
    /// `STRING` cannot — unlike a member's value).
    pub(crate) fn is_name(self) -> bool {
        matches!(self, Self::Text) || self.is_keyword()
    }

    /// True if the token can name a field after a `.`: a value, a keyword, or a
    /// number (e.g. `expr.type_map.1.type`).
    pub(crate) fn is_field(self) -> bool {
        self.is_value() || self.is_keyword() || self == Self::Number
    }

    /// True for the reserved keywords `NOT`, `AND`, `OR`.
    pub(crate) fn is_keyword(self) -> bool {
        matches!(self, Self::Not | Self::And | Self::Or)
    }

    /// The keyword token for an exact-case text value, if any.
    pub(crate) fn keyword(text: &str) -> Option<Self> {
        match text {
            "NOT" => Some(Self::Not),
            "AND" => Some(Self::And),
            "OR" => Some(Self::Or),
            _ => None,
        }
    }
}

/// A single lexed token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    /// The token kind.
    pub(crate) ty: TokenType,
    /// Byte offset of the token's first character in the original filter.
    pub(crate) offset: usize,
    /// The raw slice of the filter the token spans (quotes included for strings).
    pub(crate) value: String,
}

/// Unquotes and unescapes a string token's raw value (surrounding quotes
/// included) into its logical value. Escaping is GoogleSQL-compatible, adapted
/// from CEL's `unescape`.
///
/// Returns the failure message (without a position) on a malformed escape; the
/// caller attaches the token's position.
pub(crate) fn unescape(value: &str) -> Result<String, String> {
    // All strings normalize newlines to `\n`.
    let value = value.replace("\r\n", "\n").replace('\r', "\n");
    let bytes = value.as_bytes();
    let n = bytes.len();
    if n < 2 {
        return Err("unable to unescape string".to_string());
    }
    // A quoted string must open and close with the same quote character.
    if bytes[0] != bytes[n - 1] || (bytes[0] != b'"' && bytes[0] != b'\'') {
        return Err("unable to unescape string".to_string());
    }
    // Quotes are ASCII, so 1 and n-1 are char boundaries.
    let inner = &value[1..n - 1];
    if !inner.contains('\\') {
        return Ok(inner.to_string());
    }
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while !rest.is_empty() {
        let (c, tail) = unescape_char(rest)?;
        out.push(c);
        rest = tail;
    }
    Ok(out)
}

/// Decodes the escape (or plain character) at the front of `s`, returning the
/// decoded character and the remainder of the string.
fn unescape_char(s: &str) -> Result<(char, &str), String> {
    let c = s.chars().next().expect("caller guarantees s is non-empty");
    if c != '\\' {
        return Ok((c, &s[c.len_utf8()..]));
    }
    let after = &s[1..];
    let Some(e) = after.chars().next() else {
        return Err("unable to unescape string, found '\\' as last character".to_string());
    };
    let tail = &after[e.len_utf8()..];
    match e {
        'a' => Ok(('\u{07}', tail)),
        'b' => Ok(('\u{08}', tail)),
        'f' => Ok(('\u{0C}', tail)),
        'n' => Ok(('\n', tail)),
        'r' => Ok(('\r', tail)),
        't' => Ok(('\t', tail)),
        'v' => Ok(('\u{0B}', tail)),
        '\\' => Ok(('\\', tail)),
        '\'' => Ok(('\'', tail)),
        '"' => Ok(('"', tail)),
        '`' => Ok(('`', tail)),
        '?' => Ok(('?', tail)),
        'x' | 'X' => hex_escape(tail, 2),
        'u' => hex_escape(tail, 4),
        'U' => hex_escape(tail, 8),
        '0'..='3' => octal_escape(e, tail),
        _ => Err("unable to unescape string".to_string()),
    }
}

/// Reads exactly `n` hex digits from the front of `s` as a Unicode code point.
fn hex_escape(s: &str, n: usize) -> Result<(char, &str), String> {
    let mut v: u32 = 0;
    let mut consumed = 0usize;
    let mut chars = s.chars();
    for _ in 0..n {
        let ch = chars
            .next()
            .ok_or_else(|| "unable to unescape string".to_string())?;
        let digit = ch
            .to_digit(16)
            .ok_or_else(|| "unable to unescape string".to_string())?;
        v = (v << 4) | digit;
        consumed += ch.len_utf8();
    }
    let c = char::from_u32(v).ok_or_else(|| "invalid unicode code point".to_string())?;
    Ok((c, &s[consumed..]))
}

/// Reads the two octal digits following `first` (`\[0-3][0-7][0-7]`) as a
/// Unicode code point.
fn octal_escape(first: char, s: &str) -> Result<(char, &str), String> {
    let mut v: u32 = first as u32 - '0' as u32;
    let mut consumed = 0usize;
    let mut chars = s.chars();
    for _ in 0..2 {
        let ch = chars
            .next()
            .ok_or_else(|| "unable to unescape octal sequence in string".to_string())?;
        if !('0'..='7').contains(&ch) {
            return Err("unable to unescape octal sequence in string".to_string());
        }
        v = v * 8 + (ch as u32 - '0' as u32);
        consumed += ch.len_utf8();
    }
    let c = char::from_u32(v).ok_or_else(|| "invalid unicode code point".to_string())?;
    Ok((c, &s[consumed..]))
}
