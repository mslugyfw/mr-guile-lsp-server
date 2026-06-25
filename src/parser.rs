//! Minimal S-expression parser.
//!
//! Used to parse the Guile REPL's `write` output (completions, documentation,
//! locations) into a Rust-friendly tree, and later for structural source
//! analysis (document symbols, use-modules extraction).
//!
//! Format handled: symbols, string literals (with escapes), numbers, `#t`/`#f`,
//! proper lists `()`, nested lists, dotted pairs / improper lists `(a . b)` and
//! alists `(("file" . "/x") ("line" . 5))`.

/// A parsed S-expression.
#[derive(Debug, Clone, PartialEq)]
pub enum SExpr {
    /// A proper list: `(a b c)`.
    List(Vec<SExpr>),
    /// A symbol or identifier: `display`, `+`, `greet`.
    Symbol(String),
    /// A string literal: `"hello"`.
    Str(String),
    /// A number: `42`, `-3.14`.
    Number(f64),
    /// `#t` or `#f`.
    Bool(bool),
    /// An improper list with a dotted tail: `(a b . c)` = head `[a, b]`, tail `c`.
    Improper(Vec<SExpr>, Box<SExpr>),
}

impl SExpr {
    /// Convenience: true if this is a `Bool(false)`.
    pub fn is_false(&self) -> bool {
        matches!(self, SExpr::Bool(false))
    }

    /// If this is a `Str`, return its contents.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SExpr::Str(s) => Some(s),
            _ => None,
        }
    }

    /// If this is a proper `List`, return its elements.
    pub fn as_list(&self) -> Option<&[SExpr]> {
        match self {
            SExpr::List(v) => Some(v),
            _ => None,
        }
    }

    /// Treat this as an association list (a list of pairs) and return the
    /// value bound to `key`. Keys may be symbols or strings, case-sensitive.
    pub fn alist_ref(&self, key: &str) -> Option<&SExpr> {
        let entries = self.as_list()?;
        for entry in entries {
            let (head, tail) = match entry {
                SExpr::Improper(head, tail) => (head.as_slice(), tail.as_ref()),
                // A 2-element proper list (key value) also acts as a pair.
                SExpr::List(v) if v.len() == 2 => (&v[..1], &v[1]),
                _ => continue,
            };
            let matches = match head.first()? {
                SExpr::Symbol(s) => s == key,
                SExpr::Str(s) => s == key,
                _ => false,
            };
            if matches {
                return Some(tail);
            }
        }
        None
    }
}

/// Errors raised while parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    Unexpected(String),
    UnterminatedString,
    UnterminatedList,
    Eof,
}

/// Parse a single S-expression from the input (trailing whitespace tolerated).
pub fn parse(input: &str) -> Result<SExpr, ParseError> {
    let mut p = Parser::new(input);
    p.skip_ws();
    let expr = p.parse_expr()?;
    Ok(expr)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser {
            bytes: s.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else if c == b';' {
                // line comment to end of line
                while let Some(cc) = self.peek() {
                    self.pos += 1;
                    if cc == b'\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn parse_expr(&mut self) -> Result<SExpr, ParseError> {
        self.skip_ws();
        match self.peek() {
            None => Err(ParseError::Eof),
            Some(b'(') => self.parse_list(),
            Some(b'"') => self.parse_string(),
            Some(b'#') => self.parse_hash(),
            Some(_) => self.parse_atom(),
        }
    }

    /// Read an atom (number or symbol): everything up to the next delimiter,
    /// then classify it.
    fn parse_atom(&mut self) -> Result<SExpr, ParseError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'"' | b';') {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(ParseError::Unexpected("empty atom".into()));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        // Try to classify as a number (int or float, optional sign).
        if let Ok(n) = text.parse::<f64>() {
            // Reject things f64 would accept that aren't Scheme numbers, e.g.
            // bare "+" or "-" parse to NaN-ish — parse::<f64> rejects those,
            // so this is safe.
            return Ok(SExpr::Number(n));
        }
        Ok(SExpr::Symbol(text.to_string()))
    }

    /// Parse a `"..."` string literal with escapes.
    fn parse_string(&mut self) -> Result<SExpr, ParseError> {
        // consume opening quote
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(ParseError::UnterminatedString),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(SExpr::Str(out));
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'n') => out.push('\n'),
                        Some(b't') => out.push('\t'),
                        Some(b'r') => out.push('\r'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'"') => out.push('"'),
                        Some(other) => out.push(other as char),
                        None => return Err(ParseError::UnterminatedString),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    // copy one UTF-8 char
                    let ch_start = self.pos;
                    let first = self.bytes[self.pos];
                    let len = utf8_len(first);
                    self.pos += len;
                    out.push_str(std::str::from_utf8(&self.bytes[ch_start..self.pos]).unwrap());
                }
            }
        }
    }

    /// Parse `#t` / `#f` (and `#true` / `#false`).
    fn parse_hash(&mut self) -> Result<SExpr, ParseError> {
        self.pos += 1; // consume '#'
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'"' | b';') {
                break;
            }
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        match text {
            "t" | "true" => Ok(SExpr::Bool(true)),
            "f" | "false" => Ok(SExpr::Bool(false)),
            other => Err(ParseError::Unexpected(format!("#{other}"))),
        }
    }

    /// Parse a list `(a b c)` or dotted/improper list `(a b . c)`.
    fn parse_list(&mut self) -> Result<SExpr, ParseError> {
        self.pos += 1; // consume '('
        let mut elems = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(ParseError::UnterminatedList),
                Some(b')') => {
                    self.pos += 1;
                    return Ok(SExpr::List(elems));
                }
                Some(b'.') if is_dotted_marker(self.bytes, self.pos) => {
                    self.pos += 1; // consume '.'
                    self.skip_ws();
                    let tail = self.parse_expr()?;
                    self.skip_ws();
                    if self.peek() != Some(b')') {
                        return Err(ParseError::Unexpected(
                            "expected ')' after dotted tail".into(),
                        ));
                    }
                    self.pos += 1; // consume ')'
                    return Ok(SExpr::Improper(elems, Box::new(tail)));
                }
                Some(_) => elems.push(self.parse_expr()?),
            }
        }
    }
}

/// True if the `.` at `pos` is a dotted-pair marker (followed by a delimiter).
fn is_dotted_marker(bytes: &[u8], pos: usize) -> bool {
    if bytes.get(pos) != Some(&b'.') {
        return false;
    }
    match bytes.get(pos + 1) {
        None => true,
        Some(c) => c.is_ascii_whitespace() || matches!(c, b')' | b'(' | b'"' | b';'),
    }
}

/// Leading-byte → UTF-8 sequence length (1..4), assuming valid UTF-8 input.
fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_symbol() {
        assert_eq!(parse("display"), Ok(SExpr::Symbol("display".to_string())));
    }

    #[test]
    fn parses_a_string() {
        assert_eq!(parse("\"hello\""), Ok(SExpr::Str("hello".to_string())));
    }

    #[test]
    fn parses_a_string_with_escapes() {
        // Guile's write escapes: \" \\ \n
        assert_eq!(
            parse(r#""a\"b\\c\nd""#),
            Ok(SExpr::Str("a\"b\\c\nd".to_string()))
        );
    }

    #[test]
    fn parses_an_integer() {
        assert_eq!(parse("42"), Ok(SExpr::Number(42.0)));
    }

    #[test]
    fn parses_a_negative_float() {
        assert_eq!(parse("-2.5"), Ok(SExpr::Number(-2.5)));
    }

    #[test]
    fn parses_true_and_false() {
        assert_eq!(parse("#t"), Ok(SExpr::Bool(true)));
        assert_eq!(parse("#f"), Ok(SExpr::Bool(false)));
    }

    #[test]
    fn parses_empty_list() {
        assert_eq!(parse("()"), Ok(SExpr::List(vec![])));
    }

    #[test]
    fn parses_simple_list() {
        assert_eq!(
            parse("(display map fold)"),
            Ok(SExpr::List(vec![
                SExpr::Symbol("display".into()),
                SExpr::Symbol("map".into()),
                SExpr::Symbol("fold".into()),
            ]))
        );
    }

    #[test]
    fn parses_nested_list() {
        assert_eq!(
            parse("(a (b c) d)"),
            Ok(SExpr::List(vec![
                SExpr::Symbol("a".into()),
                SExpr::List(vec![SExpr::Symbol("b".into()), SExpr::Symbol("c".into()),]),
                SExpr::Symbol("d".into()),
            ]))
        );
    }

    #[test]
    fn parses_dotted_pair_alist() {
        // Geiser location format: (("file" . "/x") ("line" . 5))
        assert_eq!(
            parse(r#"(("file" . "/x") ("line" . 5))"#),
            Ok(SExpr::List(vec![
                SExpr::Improper(
                    vec![SExpr::Str("file".into())],
                    Box::new(SExpr::Str("/x".into())),
                ),
                SExpr::Improper(
                    vec![SExpr::Str("line".into())],
                    Box::new(SExpr::Number(5.0)),
                ),
            ]))
        );
    }

    #[test]
    fn parses_utf8_string_content() {
        // Guile docstrings can contain Chinese; the parser must copy whole chars.
        assert_eq!(
            parse(r#""问候函数""#),
            Ok(SExpr::Str("问候函数".to_string()))
        );
    }

    #[test]
    fn alist_ref_finds_symbol_and_string_keys() {
        // ((warnings . "text") ("error" . #f))
        let expr = parse(r#"(("warnings" . "text") ("error" . #f))"#).unwrap();
        assert_eq!(
            expr.alist_ref("warnings").and_then(|v| v.as_str()),
            Some("text")
        );
        assert!(expr
            .alist_ref("error")
            .map(|v| v.is_false())
            .unwrap_or(false));
        assert!(expr.alist_ref("missing").is_none());
    }
}
