//! Tokens of the Thrift interface definition language.
//!
//! The lexer never fails: malformed input becomes a [`TokenKind::Error`]
//! token carrying a message, and the parser reports it where it appears.
//! Comments (`//`, `#` and `/* */`) are skipped, except that a `/** */`
//! comment directly before a definition is kept as its documentation.

use faultline_source::{SourceId, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// An identifier, possibly dotted (`shared.Thing`).
    Ident(String),
    /// An integer literal, decimal or hexadecimal (`0x1F`), with its sign.
    Int(i64),
    /// A floating-point literal (`1.5`, `-1e10`).
    Double(f64),
    /// A string literal, in single or double quotes, escapes decoded.
    Str(String),
    Symbol(char),
    /// Input the lexer could not read.
    Error(String),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// The text of a `/** */` comment directly before the token.
    pub doc: Option<String>,
}

/// Splits `text` into tokens, ending with [`TokenKind::Eof`].
pub fn tokenize(source: SourceId, text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        source,
        text,
        bytes: text.as_bytes(),
        pos: 0,
        tokens: Vec::new(),
        doc: None,
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'t> {
    source: SourceId,
    text: &'t str,
    bytes: &'t [u8],
    pos: usize,
    tokens: Vec<Token>,
    doc: Option<String>,
}

impl Lexer<'_> {
    fn span(&self, start: usize) -> Span {
        let offset = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Span::new(self.source, offset(start), offset(self.pos))
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos + ahead).copied()
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let doc = self.doc.take();
        self.tokens.push(Token {
            kind,
            span: self.span(start),
            doc,
        });
    }

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek(0) else {
                self.push(TokenKind::Eof, start);
                return;
            };
            match c {
                b'"' | b'\'' => self.string(c),
                b'0'..=b'9' => self.number(start),
                b'+' | b'-'
                    if self
                        .peek(1)
                        .is_some_and(|d| d.is_ascii_digit() || d == b'.') =>
                {
                    self.number(start);
                }
                b'.' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => self.number(start),
                c if c.is_ascii_alphabetic() || c == b'_' => self.ident(start),
                _ => {
                    let ch = self.text[start..].chars().next().expect("not at the end");
                    self.pos += ch.len_utf8();
                    let kind = if "{}()<>[],;:=*".contains(ch) {
                        TokenKind::Symbol(ch)
                    } else {
                        TokenKind::Error(format!("unexpected {ch:?}"))
                    };
                    self.push(kind, start);
                }
            }
        }
    }

    /// Skips whitespace and comments, keeping the last doc comment.
    fn skip_trivia(&mut self) {
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(c), _) if c.is_ascii_whitespace() => self.pos += 1,
                (Some(b'/'), Some(b'/')) | (Some(b'#'), _) => {
                    while self.peek(0).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let start = self.pos;
                    let doc = self.peek(2) == Some(b'*') && self.peek(3) != Some(b'/');
                    self.pos += 2;
                    loop {
                        match (self.peek(0), self.peek(1)) {
                            (Some(b'*'), Some(b'/')) => {
                                self.pos += 2;
                                break;
                            }
                            (Some(_), _) => self.pos += 1,
                            (None, _) => {
                                self.push(TokenKind::Error("unterminated comment".into()), start);
                                return;
                            }
                        }
                    }
                    self.doc = doc.then(|| doc_text(&self.text[start + 3..self.pos - 2]));
                }
                _ => return,
            }
        }
    }

    fn ident(&mut self, start: usize) {
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
        {
            self.pos += 1;
        }
        let text = self.text[start..self.pos].to_owned();
        self.push(TokenKind::Ident(text), start);
    }

    fn number(&mut self, start: usize) {
        if matches!(self.peek(0), Some(b'+' | b'-')) {
            self.pos += 1;
        }
        let negative = self.bytes[start] == b'-';
        let digits_start = self.pos;
        if self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X')) {
            self.pos += 2;
            let hex_start = self.pos;
            while self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let digits = &self.text[hex_start..self.pos];
            let kind = match u64::from_str_radix(digits, 16) {
                Ok(n) => signed(n, negative),
                Err(_) => TokenKind::Error(format!("malformed hexadecimal literal 0x{digits}")),
            };
            self.push(kind, start);
            return;
        }
        let mut float = false;
        while let Some(c) = self.peek(0) {
            match c {
                b'0'..=b'9' => self.pos += 1,
                b'.' if !float => {
                    float = true;
                    self.pos += 1;
                }
                b'e' | b'E' => {
                    float = true;
                    self.pos += 1;
                    if matches!(self.peek(0), Some(b'+' | b'-')) {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        let text = &self.text[start..self.pos];
        let kind = if float {
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() => TokenKind::Double(value),
                _ => TokenKind::Error(format!("malformed number {text}")),
            }
        } else {
            match self.text[digits_start..self.pos].parse::<u64>() {
                Ok(n) => signed(n, negative),
                Err(_) => TokenKind::Error(format!("integer {text} is out of range")),
            }
        };
        self.push(kind, start);
    }

    fn string(&mut self, quote: u8) {
        let start = self.pos;
        self.pos += 1;
        let mut value = String::new();
        loop {
            let Some(c) = self.text[self.pos..].chars().next() else {
                self.push(TokenKind::Error("unterminated string".into()), start);
                return;
            };
            self.pos += c.len_utf8();
            match c {
                _ if c as u32 == u32::from(quote) => break,
                '\\' => {
                    let Some(escaped) = self.text[self.pos..].chars().next() else {
                        continue;
                    };
                    self.pos += escaped.len_utf8();
                    value.push(match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                }
                _ => value.push(c),
            }
        }
        self.push(TokenKind::Str(value), start);
    }
}

fn signed(magnitude: u64, negative: bool) -> TokenKind {
    let value = if negative {
        0i64.checked_sub_unsigned(magnitude)
    } else {
        i64::try_from(magnitude).ok()
    };
    value.map_or_else(
        || TokenKind::Error("integer literal out of the 64-bit range".into()),
        TokenKind::Int,
    )
}

/// The text of a doc comment without its leading `*`s and indentation.
fn doc_text(body: &str) -> String {
    body.lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .skip_while(|line| line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<TokenKind> {
        tokenize(SourceId::from_raw(0), text)
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn ident(s: &str) -> TokenKind {
        TokenKind::Ident(s.into())
    }

    #[test]
    fn tokens() {
        assert_eq!(
            kinds("struct Pet { 1: required i64 id = -1, 2: shared.Tag tag; }"),
            [
                ident("struct"),
                ident("Pet"),
                TokenKind::Symbol('{'),
                TokenKind::Int(1),
                TokenKind::Symbol(':'),
                ident("required"),
                ident("i64"),
                ident("id"),
                TokenKind::Symbol('='),
                TokenKind::Int(-1),
                TokenKind::Symbol(','),
                TokenKind::Int(2),
                TokenKind::Symbol(':'),
                ident("shared.Tag"),
                ident("tag"),
                TokenKind::Symbol(';'),
                TokenKind::Symbol('}'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn literals() {
        assert_eq!(
            kinds(r#"0x1F 1.5 -2e3 'it\'s' "a\"b\n""#),
            [
                TokenKind::Int(31),
                TokenKind::Double(1.5),
                TokenKind::Double(-2000.0),
                TokenKind::Str("it's".into()),
                TokenKind::Str("a\"b\n".into()),
                TokenKind::Eof,
            ]
        );
        assert_eq!(
            kinds("-9223372036854775808"),
            [TokenKind::Int(i64::MIN), TokenKind::Eof]
        );
        assert!(matches!(
            kinds("9223372036854775808")[0],
            TokenKind::Error(_)
        ));
        assert!(matches!(kinds("'open")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("/* open")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("@")[0], TokenKind::Error(_)));
    }

    #[test]
    fn comments_and_docs() {
        let tokens = tokenize(
            SourceId::from_raw(0),
            "# hash\n// line\n/* block */\n/**\n * A pet.\n * Friendly.\n */\nstruct",
        );
        assert_eq!(tokens[0].kind, ident("struct"));
        assert_eq!(tokens[0].doc.as_deref(), Some("A pet.\nFriendly."));
        let tokens = tokenize(SourceId::from_raw(0), "/**/ x");
        assert_eq!(tokens[0].doc, None, "an empty block comment is not a doc");
    }
}
