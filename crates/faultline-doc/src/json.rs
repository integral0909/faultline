//! A JSON reader (RFC 8259) that records spans and produces actionable errors.
//!
//! `serde_json` discards positions, and schema diagnostics are only useful if
//! they point at the offending key. This reader is a straightforward
//! recursive-descent parser over bytes; recursion depth is bounded by
//! [`Limits::max_depth`](faultline_source::Limits::max_depth), clamped to
//! [`HARD_MAX_DEPTH`](crate::HARD_MAX_DEPTH) so a permissive configuration
//! cannot overflow the stack.
//!
//! The first syntax error is fatal (JSON has no meaningful recovery points).
//! Duplicate keys and lone surrogate escapes are reported but do not stop
//! parsing, so the caller sees every such problem at once.

use faultline_source::{Diagnostic, Diagnostics, Limits, SourceId, Span};

use crate::codes;
use crate::node::{Node, Object, Value};
use crate::number::{Decimal, DecimalError, Number};

/// Marker for a fatal error whose diagnostic has already been recorded.
struct Fatal;

type PResult<T> = Result<T, Fatal>;

pub(crate) fn parse(
    text: &str,
    source: SourceId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Option<Node> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        source,
        max_depth: limits.max_depth.min(crate::HARD_MAX_DEPTH),
        max_nodes: limits.max_document_nodes,
        depth: 0,
        nodes: 0,
        diagnostics,
    };
    parser.document().ok()
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    source: SourceId,
    max_depth: u32,
    max_nodes: u64,
    depth: u32,
    nodes: u64,
    diagnostics: &'a mut Diagnostics,
}

impl Parser<'_> {
    fn document(&mut self) -> PResult<Node> {
        self.skip_whitespace();
        if self.at_end() {
            return self.fail(
                Diagnostic::error("expected a JSON value, found end of input")
                    .with_code(codes::JSON_UNEXPECTED_END)
                    .with_primary(self.span_here(), "document is empty"),
            );
        }
        let root = self.value()?;
        self.skip_whitespace();
        if !self.at_end() {
            let span = self.span_to(self.bytes.len());
            return self.fail(
                Diagnostic::error("unexpected content after the JSON value")
                    .with_code(codes::JSON_TRAILING_CONTENT)
                    .with_primary(span, "expected end of input")
                    .with_secondary(root.span, "the document ends after this value"),
            );
        }
        Ok(root)
    }

    fn value(&mut self) -> PResult<Node> {
        self.count_node()?;
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let (s, span) = self.string()?;
                Ok(Node::new(Value::String(s), span))
            }
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.unexpected("a value"),
        }
    }

    fn object(&mut self) -> PResult<Node> {
        let start = self.pos;
        self.enter()?;
        self.pos += 1;
        let mut object = Object::new();
        self.skip_whitespace();
        if self.eat(b'}') {
            self.depth -= 1;
            return Ok(Node::new(Value::Object(object), self.span_from(start)));
        }
        loop {
            if self.peek() != Some(b'"') {
                return self.unexpected("a string key");
            }
            let (key, key_span) = self.string()?;
            self.skip_whitespace();
            if !self.eat(b':') {
                return self.unexpected("':'");
            }
            self.skip_whitespace();
            let value = self.value()?;
            if let Some(previous) = object.insert(key.clone(), key_span, value) {
                self.diagnostics.push(
                    Diagnostic::error(format!("duplicate key \"{key}\""))
                        .with_code(codes::DUPLICATE_KEY)
                        .with_primary(key_span, "duplicate key")
                        .with_secondary(previous.key_span, "first defined here")
                        .with_note("duplicate keys make the document ambiguous; parsers disagree on which value wins"),
                );
            }
            self.skip_whitespace();
            if self.eat(b'}') {
                break;
            }
            let comma = self.pos;
            if !self.eat(b',') {
                return self.unexpected("',' or '}'");
            }
            self.skip_whitespace();
            if self.peek() == Some(b'}') {
                return self.trailing_comma(comma);
            }
        }
        self.depth -= 1;
        Ok(Node::new(Value::Object(object), self.span_from(start)))
    }

    fn array(&mut self) -> PResult<Node> {
        let start = self.pos;
        self.enter()?;
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.eat(b']') {
            self.depth -= 1;
            return Ok(Node::new(Value::Array(items), self.span_from(start)));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            if self.eat(b']') {
                break;
            }
            let comma = self.pos;
            if !self.eat(b',') {
                return self.unexpected("',' or ']'");
            }
            self.skip_whitespace();
            if self.peek() == Some(b']') {
                return self.trailing_comma(comma);
            }
        }
        self.depth -= 1;
        Ok(Node::new(Value::Array(items), self.span_from(start)))
    }

    /// Parses a string starting at the opening quote.
    fn string(&mut self) -> PResult<(String, Span)> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        let mut run_start = self.pos;
        loop {
            let Some(b) = self.peek() else {
                return self.fail(
                    Diagnostic::error("unterminated string")
                        .with_code(codes::JSON_UNEXPECTED_END)
                        .with_primary(self.span_here(), "expected '\"'")
                        .with_secondary(
                            Span::new(self.source, start as u32, start as u32 + 1),
                            "string starts here",
                        ),
                );
            };
            match b {
                b'"' => {
                    out.push_str(&self.text[run_start..self.pos]);
                    self.pos += 1;
                    return Ok((out, self.span_from(start)));
                }
                b'\\' => {
                    out.push_str(&self.text[run_start..self.pos]);
                    self.escape(&mut out)?;
                    run_start = self.pos;
                }
                0x00..=0x1F => {
                    let span = self.span_to(self.pos + 1);
                    return self.fail(
                        Diagnostic::error("control character in string")
                            .with_code(codes::JSON_CONTROL_CHARACTER)
                            .with_primary(span, format!("U+{b:04X} must be escaped"))
                            .with_help("use an escape sequence such as \\n or \\t, or \\u00XX"),
                    );
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Parses an escape sequence starting at the backslash.
    fn escape(&mut self, out: &mut String) -> PResult<()> {
        let start = self.pos;
        let c = match self.bytes.get(start + 1) {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.pos += 2;
                return self.unicode_escape(start, out);
            }
            _ => {
                let end = (start + 1 + self.char_len_at(start + 1)).min(self.bytes.len());
                let span = self.span_between(start, end);
                return self.fail(
                    Diagnostic::error("invalid escape sequence")
                        .with_code(codes::JSON_INVALID_ESCAPE)
                        .with_primary(span, "not a valid escape")
                        .with_help(
                            "valid escapes are \\\" \\\\ \\/ \\b \\f \\n \\r \\t and \\uXXXX",
                        ),
                );
            }
        };
        out.push(c);
        self.pos += 2;
        Ok(())
    }

    /// Parses the hex digits of a `\u` escape (and a following low surrogate
    /// escape if needed). `start` is the position of the backslash.
    fn unicode_escape(&mut self, start: usize, out: &mut String) -> PResult<()> {
        let high = self.hex4(start)?;
        if (0xD800..0xDC00).contains(&high) {
            if self.bytes.get(self.pos) == Some(&b'\\')
                && self.bytes.get(self.pos + 1) == Some(&b'u')
            {
                let low_start = self.pos;
                self.pos += 2;
                let low = self.hex4(low_start)?;
                if (0xDC00..0xE000).contains(&low) {
                    let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                    out.push(char::from_u32(code).expect("valid surrogate pair"));
                    return Ok(());
                }
                self.lone_surrogate(start, low_start);
                out.push(char::REPLACEMENT_CHARACTER);
                // The second escape is processed on its own.
                self.push_code_unit(low, low_start, out);
                return Ok(());
            }
            self.lone_surrogate(start, self.pos);
            out.push(char::REPLACEMENT_CHARACTER);
            return Ok(());
        }
        self.push_code_unit(high, start, out);
        Ok(())
    }

    fn push_code_unit(&mut self, unit: u32, start: usize, out: &mut String) {
        if let Some(c) = char::from_u32(unit) {
            out.push(c);
        } else {
            // A low surrogate without a preceding high surrogate.
            self.lone_surrogate(start, start + 6);
            out.push(char::REPLACEMENT_CHARACTER);
        }
    }

    fn lone_surrogate(&mut self, start: usize, end: usize) {
        let span = self.span_between(start, end);
        self.diagnostics.push(
            Diagnostic::warning("unpaired UTF-16 surrogate in \\u escape")
                .with_code(codes::JSON_LONE_SURROGATE)
                .with_primary(span, "replaced with U+FFFD")
                .with_note("surrogates must appear as a high/low pair to encode one character"),
        );
    }

    fn hex4(&mut self, escape_start: usize) -> PResult<u32> {
        let digits = self.bytes.get(self.pos..self.pos + 4);
        let value = digits.and_then(|d| {
            d.iter().try_fold(0u32, |acc, &b| {
                let v = (b as char).to_digit(16)?;
                Some(acc << 4 | v)
            })
        });
        if let Some(v) = value {
            self.pos += 4;
            return Ok(v);
        }
        let end = (self.pos + 4).min(self.bytes.len());
        let end = self.floor_boundary(end);
        let span = self.span_between(escape_start, end);
        self.fail(
            Diagnostic::error("invalid unicode escape")
                .with_code(codes::JSON_INVALID_ESCAPE)
                .with_primary(span, "expected four hexadecimal digits after \\u"),
        )
    }

    fn number(&mut self) -> PResult<Node> {
        let start = self.pos;
        self.eat(b'-');
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return self.invalid_number(start, "leading zeros are not allowed");
                }
            }
            Some(b'1'..=b'9') => self.skip_digits(),
            _ => return self.invalid_number(start, "expected a digit"),
        }
        if self.eat(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.invalid_number(start, "expected a digit after the decimal point");
            }
            self.skip_digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return self.invalid_number(start, "expected a digit in the exponent");
            }
            self.skip_digits();
        }
        let span = self.span_from(start);
        match Decimal::parse(&self.text[start..self.pos]) {
            Ok(d) => Ok(Node::new(Value::Number(Number::Finite(d)), span)),
            Err(DecimalError::ExponentOutOfRange) => self.fail(
                Diagnostic::error("number is out of range")
                    .with_code(codes::NUMBER_OUT_OF_RANGE)
                    .with_primary(span, "exponent is too large"),
            ),
            Err(DecimalError::Syntax) => unreachable!("grammar validated above"),
        }
    }

    fn invalid_number<T>(&mut self, start: usize, message: &str) -> PResult<T> {
        // Extend the span over the rest of the token for a readable underline.
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        ) {
            self.pos += 1;
        }
        let span = self.span_from(start);
        self.fail(
            Diagnostic::error("invalid number")
                .with_code(codes::JSON_INVALID_NUMBER)
                .with_primary(span, message),
        )
    }

    fn skip_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> PResult<Node> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            let start = self.pos;
            self.pos += word.len();
            return Ok(Node::new(value, self.span_from(start)));
        }
        self.unexpected("a value")
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > self.max_depth {
            let span = self.span_to(self.pos + 1);
            return self.fail(
                Diagnostic::error("document is nested too deeply")
                    .with_code(codes::DEPTH_LIMIT)
                    .with_primary(
                        span,
                        format!("exceeds the nesting limit of {}", self.max_depth),
                    ),
            );
        }
        Ok(())
    }

    fn count_node(&mut self) -> PResult<()> {
        self.nodes += 1;
        if self.nodes > self.max_nodes {
            let span = self.span_to(self.pos + 1);
            return self.fail(
                Diagnostic::error("document has too many values")
                    .with_code(codes::NODE_LIMIT)
                    .with_primary(
                        span,
                        format!("exceeds the limit of {} values", self.max_nodes),
                    ),
            );
        }
        Ok(())
    }

    fn trailing_comma<T>(&mut self, comma: usize) -> PResult<T> {
        let span = self.span_between(comma, comma + 1);
        self.fail(
            Diagnostic::error("trailing comma")
                .with_code(codes::JSON_UNEXPECTED_CHARACTER)
                .with_primary(span, "JSON does not allow a comma before a closing bracket")
                .with_help("remove this comma"),
        )
    }

    /// Reports an unexpected character (or end of input) where `expected` was
    /// required, with hints for common mistakes.
    fn unexpected<T>(&mut self, expected: &str) -> PResult<T> {
        let Some(b) = self.peek() else {
            return self.fail(
                Diagnostic::error(format!("expected {expected}, found end of input"))
                    .with_code(codes::JSON_UNEXPECTED_END)
                    .with_primary(self.span_here(), format!("expected {expected}")),
            );
        };
        let len = self.char_len_at(self.pos);
        let found = &self.text[self.pos..self.pos + len];
        let span = self.span_to(self.pos + len);
        let mut diagnostic = Diagnostic::error(format!("expected {expected}, found '{found}'"))
            .with_code(codes::JSON_UNEXPECTED_CHARACTER)
            .with_primary(span, format!("expected {expected}"));
        let rest = &self.bytes[self.pos..];
        let help = match b {
            b'\'' => Some("JSON strings must use double quotes"),
            b'/' if rest.starts_with(b"//") || rest.starts_with(b"/*") => {
                Some("JSON does not support comments; use YAML if you need them")
            }
            b'N' if rest.starts_with(b"NaN") => Some("NaN is not a valid JSON number"),
            b'I' | b'-' if rest.starts_with(b"Infinity") || rest.starts_with(b"-Infinity") => {
                Some("Infinity is not a valid JSON number")
            }
            b if expected == "a string key" && (b.is_ascii_alphabetic() || b == b'_') => {
                Some("object keys must be enclosed in double quotes")
            }
            _ => None,
        };
        if let Some(help) = help {
            diagnostic = diagnostic.with_help(help);
        }
        self.fail(diagnostic)
    }

    fn fail<T>(&mut self, diagnostic: Diagnostic) -> PResult<T> {
        self.diagnostics.push(diagnostic);
        Err(Fatal)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn char_len_at(&self, pos: usize) -> usize {
        self.text
            .get(pos..)
            .and_then(|s| s.chars().next())
            .map_or(1, char::len_utf8)
    }

    fn floor_boundary(&self, mut pos: usize) -> usize {
        while !self.text.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    fn span_between(&self, start: usize, end: usize) -> Span {
        Span::new(self.source, start as u32, end as u32)
    }

    fn span_from(&self, start: usize) -> Span {
        self.span_between(start, self.pos)
    }

    fn span_to(&self, end: usize) -> Span {
        self.span_between(self.pos, end.min(self.bytes.len()))
    }

    fn span_here(&self) -> Span {
        Span::empty(self.source, self.pos as u32)
    }
}
