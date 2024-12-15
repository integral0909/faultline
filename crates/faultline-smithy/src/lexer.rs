//! Tokens of the Smithy interface definition language (IDL 2.0).
//!
//! Commas are whitespace in Smithy. `//` starts a comment, and `///` a
//! documentation comment, whose text is kept for the shape or member that
//! follows. The lexer never fails: malformed input becomes a
//! [`TokenKind::Error`] token that the parser reports where it appears.

use faultline_source::{SourceId, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    /// An identifier or shape id: `String`, `smithy.api#String`,
    /// `Pet$name`.
    Ident(String),
    /// A number, as written.
    Number(String),
    /// A string or text block, escapes decoded.
    Str(String),
    Symbol(char),
    /// `:=`, which introduces an inline structure.
    Walrus,
    Error(String),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// The text of `///` comments directly before the token.
    pub doc: Option<String>,
}

/// Splits `text` into tokens, ending with [`TokenKind::Eof`].
pub fn tokenize(source: SourceId, text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        source,
        text,
        pos: 0,
        tokens: Vec::new(),
        doc: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'t> {
    source: SourceId,
    text: &'t str,
    pos: usize,
    tokens: Vec<Token>,
    doc: Vec<String>,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.text[self.pos..].chars().nth(ahead)
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        let offset = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let doc = (!self.doc.is_empty()).then(|| std::mem::take(&mut self.doc).join("\n"));
        self.tokens.push(Token {
            kind,
            span: Span::new(self.source, offset(start), offset(self.pos)),
            doc,
        });
    }

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek() else {
                self.push(TokenKind::Eof, start);
                return;
            };
            match c {
                '"' => self.string(start),
                '-' | '0'..='9' => self.number(start),
                c if c.is_ascii_alphabetic() || c == '_' => self.ident(start),
                ':' if self.peek_at(1) == Some('=') => {
                    self.pos += 2;
                    self.push(TokenKind::Walrus, start);
                }
                _ => {
                    self.pos += c.len_utf8();
                    let kind = if "{}[]():=@$".contains(c) {
                        TokenKind::Symbol(c)
                    } else {
                        TokenKind::Error(format!("unexpected {c:?}"))
                    };
                    self.push(kind, start);
                }
            }
        }
    }

    /// Skips whitespace, commas and comments, collecting `///` text.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() || c == ',' => self.pos += c.len_utf8(),
                Some('/') if self.peek_at(1) == Some('/') => {
                    let start = self.pos;
                    let end = self.text[start..]
                        .find('\n')
                        .map_or(self.text.len(), |n| start + n);
                    let line = &self.text[start..end];
                    if let Some(doc) = line.strip_prefix("///") {
                        let doc = doc.strip_prefix(' ').unwrap_or(doc);
                        self.doc.push(doc.trim_end().to_owned());
                    }
                    self.pos = end;
                }
                _ => return,
            }
        }
    }

    fn ident(&mut self, start: usize) {
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        while let Some(c) = self.peek() {
            if word(c) || (matches!(c, '.' | '#' | '$') && self.peek_at(1).is_some_and(word)) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = self.text[start..self.pos].to_owned();
        self.push(TokenKind::Ident(text), start);
    }

    fn number(&mut self, start: usize) {
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        let digits = |lexer: &mut Self| {
            let from = lexer.pos;
            while lexer.peek().is_some_and(|c| c.is_ascii_digit()) {
                lexer.pos += 1;
            }
            lexer.pos > from
        };
        let mut valid = digits(self);
        if self.peek() == Some('.') {
            self.pos += 1;
            valid &= digits(self);
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some('+' | '-')) {
                self.pos += 1;
            }
            valid &= digits(self);
        }
        let text = self.text[start..self.pos].to_owned();
        let kind = if valid {
            TokenKind::Number(text)
        } else {
            TokenKind::Error(format!("malformed number {text}"))
        };
        self.push(kind, start);
    }

    fn string(&mut self, start: usize) {
        if self.text[self.pos..].starts_with("\"\"\"") {
            self.pos += 3;
            let kind = match self.text_block() {
                Ok(text) => TokenKind::Str(text),
                Err(message) => TokenKind::Error(message),
            };
            self.push(kind, start);
            return;
        }
        self.pos += 1;
        let kind = match self.quoted(|l| l.peek() == Some('"'), 1) {
            Ok(text) => TokenKind::Str(text),
            Err(message) => TokenKind::Error(message),
        };
        self.push(kind, start);
    }

    /// The characters up to where `at_end` says the literal ends, whose
    /// closing delimiter is `close` bytes long, with escapes decoded.
    fn quoted(&mut self, at_end: impl Fn(&Self) -> bool, close: usize) -> Result<String, String> {
        let mut out = String::new();
        loop {
            if at_end(self) {
                self.pos += close;
                return Ok(out);
            }
            let Some(c) = self.peek() else {
                return Err("unterminated string".into());
            };
            self.pos += c.len_utf8();
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(escaped) = self.peek() else {
                return Err("unterminated string".into());
            };
            self.pos += escaped.len_utf8();
            match escaped {
                '"' | '\\' | '/' | '\'' => out.push(escaped),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                // A line continuation.
                '\n' => {}
                'u' => {
                    let hex = self.text.get(self.pos..self.pos + 4).unwrap_or_default();
                    let code = u32::from_str_radix(hex, 16)
                        .ok()
                        .filter(|_| hex.len() == 4)
                        .ok_or_else(|| "expected four hexadecimal digits".to_owned())?;
                    self.pos += 4;
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => return Err(format!("invalid escape \\{other}")),
            }
        }
    }

    /// A text block after its opening `"""`: the lines up to the closing
    /// `"""`, without the indentation they share.
    fn text_block(&mut self) -> Result<String, String> {
        let rest = &self.text[self.pos..];
        let Some(first_break) = rest.find('\n') else {
            return Err("a text block starts on the line after `\"\"\"`".into());
        };
        if !rest[..first_break].trim().is_empty() {
            return Err("a text block starts on the line after `\"\"\"`".into());
        }
        self.pos += first_break + 1;
        let raw = self.quoted(|l| l.text[l.pos..].starts_with("\"\"\""), 3)?;
        Ok(strip_indentation(&raw))
    }
}

/// Removes the indentation all non-blank lines (and the line of the
/// closing delimiter) share, and trailing spaces.
fn strip_indentation(raw: &str) -> String {
    let lines: Vec<&str> = raw.split('\n').collect();
    let last = lines.len() - 1;
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| *i == last || !l.trim().is_empty())
        .map(|(_, l)| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines
        .iter()
        .map(|l| l.get(indent..).unwrap_or("").trim_end().to_owned())
        .collect();
    // The closing delimiter on its own line ends the last line.
    if out.last().is_some_and(String::is_empty) {
        out.pop();
        return out.join("\n") + "\n";
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

    fn ident(s: &str) -> TokenKind {
        TokenKind::Ident(s.into())
    }

    #[test]
    fn tokens() {
        assert_eq!(
            kinds(
                "$version: \"2\"\nstructure Pet with [Named] { @required name: smithy.api#String, id: Pet$id = -1.5e3 }\ninput := {}"
            ),
            [
                TokenKind::Symbol('$'),
                ident("version"),
                TokenKind::Symbol(':'),
                TokenKind::Str("2".into()),
                ident("structure"),
                ident("Pet"),
                ident("with"),
                TokenKind::Symbol('['),
                ident("Named"),
                TokenKind::Symbol(']'),
                TokenKind::Symbol('{'),
                TokenKind::Symbol('@'),
                ident("required"),
                ident("name"),
                TokenKind::Symbol(':'),
                ident("smithy.api#String"),
                ident("id"),
                TokenKind::Symbol(':'),
                ident("Pet$id"),
                TokenKind::Symbol('='),
                TokenKind::Number("-1.5e3".into()),
                TokenKind::Symbol('}'),
                ident("input"),
                TokenKind::Walrus,
                TokenKind::Symbol('{'),
                TokenKind::Symbol('}'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn strings_and_text_blocks() {
        assert_eq!(
            kinds(r#""a\"bé\n""#),
            [TokenKind::Str("a\"bé\n".into()), TokenKind::Eof]
        );
        let block = "\"\"\"\n    Hello\n      world\n    \"\"\"";
        assert_eq!(
            kinds(block),
            [TokenKind::Str("Hello\n  world\n".into()), TokenKind::Eof]
        );
        assert!(matches!(kinds("\"open")[0], TokenKind::Error(_)));
        assert!(matches!(
            kinds("\"\"\"same line\"\"\"")[0],
            TokenKind::Error(_)
        ));
        assert!(matches!(kinds(r#""\q""#)[0], TokenKind::Error(_)));
        assert!(matches!(kinds("1.")[0], TokenKind::Error(_)));
        assert!(matches!(kinds("%")[0], TokenKind::Error(_)));
    }

    #[test]
    fn documentation_comments() {
        let tokens = tokenize(
            SourceId::from_raw(0),
            "// not docs\n/// A pet.\n///\n/// Friendly.\nstructure Pet {}",
        );
        assert_eq!(tokens[0].doc.as_deref(), Some("A pet.\n\nFriendly."));
        assert_eq!(tokens[1].doc, None);
    }
}
