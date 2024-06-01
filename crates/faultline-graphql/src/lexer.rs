//! Tokens of the GraphQL language (October 2021 specification).
//!
//! Commas, whitespace, byte-order marks and `#` comments are ignored, as the
//! specification says. Malformed input becomes a [`TokenKind::Error`] token
//! that the parser reports; the lexer itself never fails.

use faultline_source::{SourceId, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Name(String),
    /// Integer literal text, kept as written (`-12`).
    Int(String),
    /// Float literal text, kept as written (`1.5e3`).
    Float(String),
    /// A string or block string, with escapes decoded and block-string
    /// indentation removed.
    Str(String),
    /// `...`
    Spread,
    Punct(char),
    Error(String),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub fn tokenize(source: SourceId, text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        source,
        text,
        bytes: text.as_bytes(),
        pos: 0,
        tokens: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'a> {
    source: SourceId,
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos + ahead).copied()
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        // The source map limits files to u32 offsets.
        let span = Span::new(self.source, start as u32, self.pos as u32);
        self.tokens.push(Token { kind, span });
    }

    fn run(&mut self) {
        loop {
            self.skip_ignored();
            let start = self.pos;
            let Some(b) = self.peek(0) else {
                self.push(TokenKind::Eof, start);
                return;
            };
            let kind = match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.name(),
                b'-' | b'0'..=b'9' => self.number(),
                b'"' => self.string(),
                b'.' if self.peek(1) == Some(b'.') && self.peek(2) == Some(b'.') => {
                    self.pos += 3;
                    TokenKind::Spread
                }
                b'!' | b'$' | b'&' | b'(' | b')' | b':' | b'=' | b'@' | b'[' | b']' | b'{'
                | b'|' | b'}' => {
                    self.pos += 1;
                    TokenKind::Punct(b as char)
                }
                _ => {
                    let c = self.text[self.pos..].chars().next().expect("not at end");
                    self.pos += c.len_utf8();
                    TokenKind::Error(format!("unexpected character {c:?}"))
                }
            };
            self.push(kind, start);
        }
    }

    fn skip_ignored(&mut self) {
        loop {
            match self.peek(0) {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b',') => self.pos += 1,
                Some(0xEF) if self.peek(1) == Some(0xBB) && self.peek(2) == Some(0xBF) => {
                    self.pos += 3;
                }
                Some(b'#') => {
                    while self.peek(0).is_some_and(|c| c != b'\n' && c != b'\r') {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn name(&mut self) -> TokenKind {
        let start = self.pos;
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.pos += 1;
        }
        TokenKind::Name(self.text[start..self.pos].to_owned())
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        if self.peek(0) == Some(b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        if self.digits() == 0 {
            return TokenKind::Error("expected digits".into());
        }
        if self.bytes[int_start] == b'0' && self.pos - int_start > 1 {
            return TokenKind::Error("numbers must not have leading zeros".into());
        }
        let mut float = false;
        if self.peek(0) == Some(b'.') && self.peek(1) != Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return TokenKind::Error("expected digits after `.`".into());
            }
            float = true;
        }
        if matches!(self.peek(0), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(0), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return TokenKind::Error("expected digits in the exponent".into());
            }
            float = true;
        }
        // A number directly followed by a name start or `.` is malformed.
        if self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_' || c == b'.')
        {
            while self
                .peek(0)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
            {
                self.pos += 1;
            }
            return TokenKind::Error("invalid number".into());
        }
        let text = self.text[start..self.pos].to_owned();
        if float {
            TokenKind::Float(text)
        } else {
            TokenKind::Int(text)
        }
    }

    fn string(&mut self) -> TokenKind {
        if self.peek(1) == Some(b'"') && self.peek(2) == Some(b'"') {
            return self.block_string();
        }
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(b) = self.peek(0) else {
                return TokenKind::Error("unterminated string".into());
            };
            match b {
                b'"' => {
                    self.pos += 1;
                    return TokenKind::Str(out);
                }
                b'\n' | b'\r' => return TokenKind::Error("unterminated string".into()),
                b'\\' => {
                    self.pos += 1;
                    if let Err(message) = self.escape(&mut out) {
                        while self
                            .peek(0)
                            .is_some_and(|c| !matches!(c, b'"' | b'\n' | b'\r'))
                        {
                            self.pos += 1;
                        }
                        if self.peek(0) == Some(b'"') {
                            self.pos += 1;
                        }
                        return TokenKind::Error(message);
                    }
                }
                _ => {
                    let c = self.text[self.pos..].chars().next().expect("not at end");
                    out.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), String> {
        let Some(b) = self.peek(0) else {
            return Err("unterminated string".into());
        };
        self.pos += 1;
        let c = match b {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode(out),
            _ => return Err(format!("unknown escape \\{}", b as char)),
        };
        out.push(c);
        Ok(())
    }

    /// `\uXXXX` (with surrogate pairs) or `\u{X...}`.
    fn unicode(&mut self, out: &mut String) -> Result<(), String> {
        let value = if self.peek(0) == Some(b'{') {
            self.pos += 1;
            let start = self.pos;
            while self.peek(0).is_some_and(|c| c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let hex = &self.text[start..self.pos];
            if self.peek(0) != Some(b'}') || hex.is_empty() || hex.len() > 8 {
                return Err("invalid unicode escape".into());
            }
            self.pos += 1;
            u32::from_str_radix(hex, 16).map_err(|_| "invalid unicode escape")?
        } else {
            let high = self.hex4()?;
            if (0xD800..0xDC00).contains(&high) {
                if self.peek(0) != Some(b'\\') || self.peek(1) != Some(b'u') {
                    return Err("unpaired surrogate in unicode escape".into());
                }
                self.pos += 2;
                let low = self.hex4()?;
                if !(0xDC00..0xE000).contains(&low) {
                    return Err("unpaired surrogate in unicode escape".into());
                }
                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
            } else {
                high
            }
        };
        let c = char::from_u32(value).ok_or("invalid unicode escape")?;
        out.push(c);
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let hex = self
            .text
            .get(self.pos..self.pos + 4)
            .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("\\u needs four hexadecimal digits")?;
        self.pos += 4;
        Ok(u32::from_str_radix(hex, 16).expect("four hex digits"))
    }

    fn block_string(&mut self) -> TokenKind {
        self.pos += 3;
        let mut raw = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                return TokenKind::Error("unterminated block string".into());
            }
            if self.text[self.pos..].starts_with("\"\"\"") {
                self.pos += 3;
                return TokenKind::Str(block_string_value(&raw));
            }
            if self.text[self.pos..].starts_with("\\\"\"\"") {
                raw.push_str("\"\"\"");
                self.pos += 4;
                continue;
            }
            let c = self.text[self.pos..].chars().next().expect("not at end");
            raw.push(c);
            self.pos += c.len_utf8();
        }
    }
}

/// The value of a block string: common indentation and blank leading and
/// trailing lines removed (the `BlockStringValue` algorithm).
pub fn block_string_value(raw: &str) -> String {
    let lines: Vec<&str> = raw
        .split("\r\n")
        .flat_map(|l| l.split(['\n', '\r']))
        .collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter_map(|line| {
            let width = line.len() - line.trim_start_matches([' ', '\t']).len();
            (width < line.len()).then_some(width)
        })
        .min()
        .unwrap_or(0);
    let mut out: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                *line
            } else {
                line.get(indent..).unwrap_or("")
            }
        })
        .collect();
    let blank = |line: &&str| line.chars().all(|c| c == ' ' || c == '\t');
    while out.first().is_some_and(blank) {
        out.remove(0);
    }
    while out.last().is_some_and(blank) {
        out.pop();
    }
    out.join("\n")
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

    fn name(s: &str) -> TokenKind {
        TokenKind::Name(s.into())
    }

    #[test]
    fn punctuation_names_and_numbers() {
        assert_eq!(
            kinds("type Pet { age(unit: Unit = YEARS): Int! } # comment\n, -1 2.5e3 ...on"),
            [
                name("type"),
                name("Pet"),
                TokenKind::Punct('{'),
                name("age"),
                TokenKind::Punct('('),
                name("unit"),
                TokenKind::Punct(':'),
                name("Unit"),
                TokenKind::Punct('='),
                name("YEARS"),
                TokenKind::Punct(')'),
                TokenKind::Punct(':'),
                name("Int"),
                TokenKind::Punct('!'),
                TokenKind::Punct('}'),
                TokenKind::Int("-1".into()),
                TokenKind::Float("2.5e3".into()),
                TokenKind::Spread,
                name("on"),
                TokenKind::Eof,
            ]
        );
        for bad in ["01", "1.", "1e", "-", "1a", "1.2.3"] {
            assert!(matches!(kinds(bad)[0], TokenKind::Error(_)), "{bad}");
        }
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(
            kinds(r#""a\n\"é\u{1F600}😀""#)[0],
            TokenKind::Str("a\n\"é😀😀".into())
        );
        assert!(matches!(kinds(r#""\uD83D x""#)[0], TokenKind::Error(_)));
        assert!(matches!(kinds("\"open\nx")[0], TokenKind::Error(_)));
        assert!(matches!(kinds(r#""\q" x"#)[0], TokenKind::Error(_)));
        assert_eq!(kinds(r#""\q" x"#)[1], name("x"));
    }

    #[test]
    fn block_strings_remove_common_indentation() {
        let text = "\"\"\"\n    A pet.\n\n      Indented.\n    Escaped: \\\"\"\"\n  \"\"\"";
        assert_eq!(
            kinds(text)[0],
            TokenKind::Str("A pet.\n\n  Indented.\nEscaped: \"\"\"".into())
        );
        assert_eq!(block_string_value("  one line  "), "  one line  ");
        assert!(matches!(
            kinds("\"\"\"never closed")[0],
            TokenKind::Error(_)
        ));
    }
}
