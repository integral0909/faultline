//! Tokens of the Protocol Buffers language.
//!
//! The lexer never fails: malformed input becomes an [`TokenKind::Error`]
//! token carrying a message, and the parser reports it where it appears.
//! Comments are skipped, except that the text of comments directly before a
//! declaration is kept for documentation.

use faultline_source::{SourceId, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    /// An integer literal, decimal, octal (`017`) or hexadecimal (`0x1F`).
    /// Magnitudes up to `u64::MAX`; the sign is a separate token.
    Int(u64),
    /// A floating-point literal (`1.5`, `1e10`, `.5`). `inf` and `nan` are
    /// identifiers.
    Float(f64),
    /// A string literal with escapes decoded, as raw bytes: Protobuf strings
    /// may contain arbitrary octets through `\xNN` and octal escapes.
    Str(Vec<u8>),
    Symbol(char),
    /// Input the lexer could not read.
    Error(String),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// Text of the comments immediately preceding the token (without the
    /// comment markers), when the token starts a line.
    pub doc: Option<String>,
}

/// Splits `text` into tokens, ending with [`TokenKind::Eof`].
pub fn tokenize(source: SourceId, text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        source,
        bytes: text.as_bytes(),
        text,
        pos: 0,
        tokens: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'a> {
    source: SourceId,
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn span(&self, start: usize) -> Span {
        // Sources are limited to u32 offsets by the source map.
        Span::new(self.source, start as u32, self.pos as u32)
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos + ahead).copied()
    }

    fn push(&mut self, kind: TokenKind, start: usize, doc: Option<String>) {
        let span = self.span(start);
        self.tokens.push(Token { kind, span, doc });
    }

    fn run(&mut self) {
        loop {
            let doc = self.skip_trivia();
            let start = self.pos;
            let Some(b) = self.peek(0) else {
                self.push(TokenKind::Eof, start, None);
                return;
            };
            let kind = match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.ident(),
                b'0'..=b'9' => self.number(),
                b'.' if self.peek(1).is_some_and(|c| c.is_ascii_digit()) => self.number(),
                b'"' | b'\'' => self.string(b),
                _ => {
                    let c = self.text[self.pos..].chars().next().expect("not at end");
                    self.pos += c.len_utf8();
                    if c.is_ascii_punctuation() {
                        TokenKind::Symbol(c)
                    } else {
                        TokenKind::Error(format!("unexpected character {c:?}"))
                    }
                }
            };
            self.push(kind, start, doc);
        }
    }

    /// Skips whitespace and comments, returning the text of the comments
    /// that directly precede the next token (no blank line in between).
    /// Comments that follow a token on its line describe that token, not
    /// the next one, and are left out.
    fn skip_trivia(&mut self) -> Option<String> {
        let mut doc: Vec<String> = Vec::new();
        let mut trailing = !self.tokens.is_empty();
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(b'\n'), _) => {
                    trailing = false;
                    self.pos += 1;
                    // A blank line detaches earlier comments.
                    let rest = &self.bytes[self.pos..];
                    let line_end = rest.iter().position(|&c| c == b'\n');
                    if line_end.is_some_and(|end| rest[..end].iter().all(u8::is_ascii_whitespace)) {
                        doc.clear();
                    }
                }
                (Some(c), _) if c.is_ascii_whitespace() => self.pos += 1,
                (Some(b'/'), Some(b'/')) => {
                    let start = self.pos + 2;
                    while self.peek(0).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                    if !trailing {
                        doc.push(self.text[start..self.pos].trim().to_owned());
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let start = self.pos + 2;
                    self.pos += 2;
                    while self.pos < self.bytes.len()
                        && !(self.peek(0) == Some(b'*') && self.peek(1) == Some(b'/'))
                    {
                        self.pos += 1;
                    }
                    let end = self.pos.min(self.bytes.len());
                    let comment = self.text[start..end]
                        .lines()
                        .map(|l| l.trim().trim_start_matches('*').trim())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        .to_owned();
                    if !trailing {
                        doc.push(comment);
                    }
                    self.pos = (self.pos + 2).min(self.bytes.len());
                }
                _ => break,
            }
        }
        let text = doc.join("\n").trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    fn ident(&mut self) -> TokenKind {
        let start = self.pos;
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        TokenKind::Ident(self.text[start..self.pos].to_owned())
    }

    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        if self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X')) {
            self.pos += 2;
            let digits = self.pos;
            while self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let hex = &self.text[digits..self.pos];
            if hex.is_empty() {
                return TokenKind::Error("hexadecimal literal without digits".into());
            }
            return self.trailing(u64::from_str_radix(hex, 16).map_or_else(
                |_| TokenKind::Error("integer literal is too large".into()),
                TokenKind::Int,
            ));
        }
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let mut float = false;
        if self.peek(0) == Some(b'.') {
            float = true;
            self.pos += 1;
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(0), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(self.peek(1), Some(b'+' | b'-')));
            if self.peek(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                float = true;
                self.pos += 1 + sign;
                while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        let text = &self.text[start..self.pos];
        let kind = if float {
            text.parse().map_or_else(
                |_| TokenKind::Error(format!("invalid number {text}")),
                TokenKind::Float,
            )
        } else if text.len() > 1 && text.starts_with('0') {
            if text.bytes().all(|c| (b'0'..=b'7').contains(&c)) {
                u64::from_str_radix(&text[1..], 8).map_or_else(
                    |_| TokenKind::Error("integer literal is too large".into()),
                    TokenKind::Int,
                )
            } else {
                TokenKind::Error(format!("invalid octal literal {text}"))
            }
        } else {
            text.parse().map_or_else(
                |_| TokenKind::Error("integer literal is too large".into()),
                TokenKind::Int,
            )
        };
        self.trailing(kind)
    }

    /// A number directly followed by letters (`12abc`) is malformed.
    fn trailing(&mut self, kind: TokenKind) -> TokenKind {
        if self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        {
            while self
                .peek(0)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                self.pos += 1;
            }
            return TokenKind::Error("a number must not be followed by letters".into());
        }
        kind
    }

    fn string(&mut self, quote: u8) -> TokenKind {
        self.pos += 1;
        let mut out = Vec::new();
        loop {
            let Some(b) = self.peek(0) else {
                return TokenKind::Error("unterminated string".into());
            };
            match b {
                b'\n' => return TokenKind::Error("unterminated string".into()),
                _ if b == quote => {
                    self.pos += 1;
                    return TokenKind::Str(out);
                }
                b'\\' => {
                    self.pos += 1;
                    if let Err(message) = self.escape(&mut out) {
                        // Skip to the end of the string to resynchronize.
                        while self.peek(0).is_some_and(|c| c != quote && c != b'\n') {
                            self.pos += 1;
                        }
                        if self.peek(0) == Some(quote) {
                            self.pos += 1;
                        }
                        return TokenKind::Error(message);
                    }
                }
                _ => {
                    out.push(b);
                    self.pos += 1;
                }
            }
        }
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        let Some(b) = self.peek(0) else {
            return Err("unterminated string".into());
        };
        self.pos += 1;
        let simple = match b {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'\\' | b'\'' | b'"' | b'?' => Some(b),
            _ => None,
        };
        if let Some(c) = simple {
            out.push(c);
            return Ok(());
        }
        match b {
            b'x' | b'X' => {
                let value = self.digits(16, 2).ok_or("\\x needs hexadecimal digits")?;
                out.push(u8::try_from(value).expect("two hex digits fit a byte"));
            }
            b'0'..=b'7' => {
                self.pos -= 1;
                let value = self.digits(8, 3).expect("at least one octal digit");
                out.push(u8::try_from(value).map_err(|_| "octal escape out of range")?);
            }
            b'u' | b'U' => {
                let width = if b == b'u' { 4 } else { 8 };
                let value = self
                    .digits(16, width)
                    .ok_or("unicode escape needs hexadecimal digits")?;
                let c = char::from_u32(value).ok_or("invalid unicode escape")?;
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            _ => return Err(format!("unknown escape \\{}", b as char)),
        }
        Ok(())
    }

    /// Reads up to `max` digits in `radix`; `None` if there are none.
    fn digits(&mut self, radix: u32, max: usize) -> Option<u32> {
        let mut value = 0u32;
        let mut count = 0;
        while count < max {
            let Some(d) = self.peek(0).and_then(|c| (c as char).to_digit(radix)) else {
                break;
            };
            value = value * radix + d;
            self.pos += 1;
            count += 1;
        }
        (count > 0).then_some(value)
    }
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
    fn identifiers_numbers_and_symbols() {
        assert_eq!(
            kinds("int32 id = 0x1F; // x\nfloat f = -1.5e3;"),
            [
                ident("int32"),
                ident("id"),
                TokenKind::Symbol('='),
                TokenKind::Int(31),
                TokenKind::Symbol(';'),
                ident("float"),
                ident("f"),
                TokenKind::Symbol('='),
                TokenKind::Symbol('-'),
                TokenKind::Float(1500.0),
                TokenKind::Symbol(';'),
                TokenKind::Eof,
            ]
        );
        assert_eq!(
            kinds("017 0 .5"),
            [
                TokenKind::Int(15),
                TokenKind::Int(0),
                TokenKind::Float(0.5),
                TokenKind::Eof
            ]
        );
        assert_eq!(kinds("18446744073709551615")[0], TokenKind::Int(u64::MAX));
        assert!(matches!(
            kinds("18446744073709551616")[0],
            TokenKind::Error(_)
        ));
        assert!(matches!(kinds("09")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("12abc")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("0x")[0], TokenKind::Error(_)));
    }

    #[test]
    fn strings_decode_escapes() {
        assert_eq!(
            kinds(r#""a\n\x41\101é" 'it''s'"#),
            [
                TokenKind::Str(b"a\nAA\xc3\xa9".to_vec()),
                TokenKind::Str(b"it".to_vec()),
                TokenKind::Str(b"s".to_vec()),
                TokenKind::Eof,
            ]
        );
        assert!(matches!(kinds("\"open")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("\"a\nb\"")[0], TokenKind::Error(_)));
        assert!(matches!(kinds(r#""\q" x"#)[0], TokenKind::Error(_)));
        assert_eq!(
            kinds(r#""\q" x"#)[1],
            ident("x"),
            "resynchronizes after a bad escape"
        );
    }

    #[test]
    fn leading_comments_become_documentation() {
        let tokens = tokenize(
            SourceId::from_raw(0),
            "// detached\n\n// The pet.\n/* More\n * text. */\nmessage Pet {}",
        );
        assert_eq!(tokens[0].kind, ident("message"));
        assert_eq!(tokens[0].doc.as_deref(), Some("The pet.\nMore\ntext."));
        assert_eq!(tokens[1].doc, None);

        let tokens = tokenize(SourceId::from_raw(0), "a; // about a\n// about b\nb");
        assert_eq!(tokens[2].kind, ident("b"));
        assert_eq!(tokens[2].doc.as_deref(), Some("about b"));
    }

    #[test]
    fn unterminated_block_comments_end_the_input() {
        assert_eq!(kinds("a /* never closed"), [ident("a"), TokenKind::Eof]);
    }

    #[test]
    fn spans_cover_tokens() {
        let tokens = tokenize(SourceId::from_raw(0), "  message");
        assert_eq!((tokens[0].span.start(), tokens[0].span.end()), (2, 9));
        let non_ascii = tokenize(SourceId::from_raw(0), "é x");
        assert!(matches!(non_ascii[0].kind, TokenKind::Error(_)));
        assert_eq!(non_ascii[1].kind, ident("x"));
    }
}
