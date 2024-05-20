//! Stable, human-readable identities for positions in an API.
//!
//! An [`IrPath`] names a position independently of file layout and source
//! spans, so it survives reformatting, file moves and `$ref` restructuring.
//! Findings, waivers and usage data are keyed by it, which makes the textual
//! form part of Faultline's public interface.
//!
//! The syntax is a sequence of *keyword segments* separated by `/`, followed
//! by *type segments* appended without separators:
//!
//! ```text
//! op[GET /pets/{}]/response[200]/body[application/json].items[].name
//! └─ keyword segments ─────────────────────────────────┘└─ type ────┘
//! ```
//!
//! | Segment | Syntax |
//! |---|---|
//! | operation, callback, webhook, schema root, named type | `op[..]`, `callback[..]`, `webhook[..]`, `schema[..]`, `type[..]` |
//! | parameter | `param[query:limit]` |
//! | request body, response, response header, response body | `request[..]`, `response[..]`, `header[..]`, `body[..]` |
//! | field | `.name`, or `."any name"` when it contains other characters |
//! | field arguments | `.name()` |
//! | array items, prefix item, contains | `[]`, `[0]`, `[contains]` |
//! | additional, pattern, property names | `{*}`, `{~"pattern"}`, `{names}` |
//! | map key, map value | `{key}`, `{value}` |
//! | union variant, allOf member, negation | `\|0`, `&0`, `!` |
//!
//! Inside `[...]` of keyword segments, `]` and `\` are escaped with `\`.

use std::fmt::{self, Write as _};
use std::str::FromStr;

use crate::graph::Edge;
use crate::surface::ParamLocation;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    Operation(String),
    Callback(String),
    Webhook(String),
    Schema(String),
    Type(String),
    Parameter(ParamLocation, String),
    RequestBody(String),
    Response(String),
    Header(String),
    Body(String),
    Field(String),
    /// The arguments of a field: `.name()`.
    Arguments(String),
    Items,
    PrefixItem(usize),
    Contains,
    Additional,
    PatternProperty(String),
    PropertyNames,
    MapKey,
    MapValue,
    Variant(usize),
    AllOf(usize),
    Not,
}

impl Segment {
    fn is_keyword(&self) -> bool {
        matches!(
            self,
            Segment::Operation(_)
                | Segment::Callback(_)
                | Segment::Webhook(_)
                | Segment::Schema(_)
                | Segment::Type(_)
                | Segment::Parameter(..)
                | Segment::RequestBody(_)
                | Segment::Response(_)
                | Segment::Header(_)
                | Segment::Body(_)
        )
    }

    /// The segment for traversing `edge`, or `None` for edges that do not
    /// change the position (`Ref` is transparent).
    pub fn from_edge(edge: Edge<'_>) -> Option<Segment> {
        Some(match edge {
            Edge::Field(name) => Segment::Field(name.to_owned()),
            Edge::Arguments(name) => Segment::Arguments(name.to_owned()),
            Edge::Additional => Segment::Additional,
            Edge::PatternProperty(_, pattern) => Segment::PatternProperty(pattern.to_owned()),
            Edge::PropertyNames => Segment::PropertyNames,
            Edge::Items => Segment::Items,
            Edge::PrefixItem(i) => Segment::PrefixItem(i),
            Edge::Contains => Segment::Contains,
            Edge::MapKey => Segment::MapKey,
            Edge::MapValue => Segment::MapValue,
            Edge::Variant(i) => Segment::Variant(i),
            Edge::AllOf(i) => Segment::AllOf(i),
            Edge::Not => Segment::Not,
            Edge::Ref => return None,
        })
    }
}

/// A position in an API. See the [module documentation](self) for the syntax.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IrPath {
    segments: Vec<Segment>,
}

impl IrPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn push(&mut self, segment: Segment) {
        self.segments.push(segment);
    }

    #[must_use]
    pub fn join(&self, segment: Segment) -> IrPath {
        let mut path = self.clone();
        path.push(segment);
        path
    }

    /// The path extended by traversing `edge`.
    #[must_use]
    pub fn join_edge(&self, edge: Edge<'_>) -> IrPath {
        match Segment::from_edge(edge) {
            Some(segment) => self.join(segment),
            None => self.clone(),
        }
    }

    pub fn parent(&self) -> Option<IrPath> {
        let (_, rest) = self.segments.split_last()?;
        Some(IrPath {
            segments: rest.to_vec(),
        })
    }

    pub fn starts_with(&self, prefix: &IrPath) -> bool {
        self.segments.starts_with(&prefix.segments)
    }
}

impl FromIterator<Segment> for IrPath {
    fn from_iter<T: IntoIterator<Item = Segment>>(iter: T) -> Self {
        Self {
            segments: iter.into_iter().collect(),
        }
    }
}

impl fmt::Display for IrPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        for (i, segment) in self.segments.iter().enumerate() {
            if segment.is_keyword() && i > 0 {
                out.push('/');
            }
            write_segment(segment, &mut out);
        }
        f.write_str(&out)
    }
}

impl fmt::Debug for IrPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IrPath({self})")
    }
}

fn write_segment(segment: &Segment, out: &mut String) {
    let keyword = |out: &mut String, name: &str, content: &str| {
        out.push_str(name);
        out.push('[');
        for c in content.chars() {
            if matches!(c, ']' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(']');
    };
    match segment {
        Segment::Operation(s) => keyword(out, "op", s),
        Segment::Callback(s) => keyword(out, "callback", s),
        Segment::Webhook(s) => keyword(out, "webhook", s),
        Segment::Schema(s) => keyword(out, "schema", s),
        Segment::Type(s) => keyword(out, "type", s),
        Segment::Parameter(loc, name) => keyword(out, "param", &format!("{loc}:{name}")),
        Segment::RequestBody(s) => keyword(out, "request", s),
        Segment::Response(s) => keyword(out, "response", s),
        Segment::Header(s) => keyword(out, "header", s),
        Segment::Body(s) => keyword(out, "body", s),
        Segment::Field(name) | Segment::Arguments(name) => {
            out.push('.');
            if is_bare_field(name) {
                out.push_str(name);
            } else {
                write_quoted(name, out);
            }
            if matches!(segment, Segment::Arguments(_)) {
                out.push_str("()");
            }
        }
        Segment::Items => out.push_str("[]"),
        Segment::PrefixItem(i) => {
            let _ = write!(out, "[{i}]");
        }
        Segment::Contains => out.push_str("[contains]"),
        Segment::Additional => out.push_str("{*}"),
        Segment::PatternProperty(pattern) => {
            out.push_str("{~");
            write_quoted(pattern, out);
            out.push('}');
        }
        Segment::PropertyNames => out.push_str("{names}"),
        Segment::MapKey => out.push_str("{key}"),
        Segment::MapValue => out.push_str("{value}"),
        Segment::Variant(i) => {
            let _ = write!(out, "|{i}");
        }
        Segment::AllOf(i) => {
            let _ = write!(out, "&{i}");
        }
        Segment::Not => out.push('!'),
    }
}

fn is_bare_field(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '$' | '@'))
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '@' | '-'))
}

fn write_quoted(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

/// Why a string is not a valid [`IrPath`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathParseError {
    pub position: usize,
    pub message: &'static str,
}

impl fmt::Display for PathParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid path at position {}: {}",
            self.position, self.message
        )
    }
}

impl std::error::Error for PathParseError {}

impl FromStr for IrPath {
    type Err = PathParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        PathParser { text, pos: 0 }.parse()
    }
}

struct PathParser<'a> {
    text: &'a str,
    pos: usize,
}

impl PathParser<'_> {
    fn parse(mut self) -> Result<IrPath, PathParseError> {
        let mut segments = Vec::new();
        while self.pos < self.text.len() {
            if !segments.is_empty() && self.peek() == Some('/') {
                self.pos += 1;
                if !self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    return self.error("expected a keyword segment after '/'");
                }
            }
            segments.push(self.segment()?);
        }
        Ok(IrPath { segments })
    }

    fn segment(&mut self) -> Result<Segment, PathParseError> {
        let Some(c) = self.peek() else {
            return self.error("unexpected end of path");
        };
        match c {
            '.' => {
                self.pos += 1;
                let name = if self.peek() == Some('"') {
                    self.quoted()?
                } else {
                    let start = self.pos;
                    while self.peek().is_some_and(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '@' | '-')
                    }) {
                        self.pos += 1;
                    }
                    let name = &self.text[start..self.pos];
                    if !is_bare_field(name) {
                        return self.error("expected a field name");
                    }
                    name.to_owned()
                };
                if self.eat("()") {
                    Ok(Segment::Arguments(name))
                } else {
                    Ok(Segment::Field(name))
                }
            }
            '[' => {
                for (literal, segment) in
                    [("[]", Segment::Items), ("[contains]", Segment::Contains)]
                {
                    if self.eat(literal) {
                        return Ok(segment);
                    }
                }
                self.pos += 1;
                let index = self.number()?;
                self.expect("]")?;
                Ok(Segment::PrefixItem(index))
            }
            '{' => {
                for (literal, segment) in [
                    ("{*}", Segment::Additional),
                    ("{names}", Segment::PropertyNames),
                    ("{key}", Segment::MapKey),
                    ("{value}", Segment::MapValue),
                ] {
                    if self.eat(literal) {
                        return Ok(segment);
                    }
                }
                self.expect("{~")?;
                let pattern = self.quoted()?;
                self.expect("}")?;
                Ok(Segment::PatternProperty(pattern))
            }
            '|' => {
                self.pos += 1;
                self.number().map(Segment::Variant)
            }
            '&' => {
                self.pos += 1;
                self.number().map(Segment::AllOf)
            }
            '!' => {
                self.pos += 1;
                Ok(Segment::Not)
            }
            c if c.is_ascii_alphabetic() => self.keyword(),
            _ => self.error("unexpected character"),
        }
    }

    fn keyword(&mut self) -> Result<Segment, PathParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        let name = &self.text[start..self.pos];
        let name_pos = start;
        self.expect("[")?;
        let content = self.bracketed()?;
        Ok(match name {
            "op" => Segment::Operation(content),
            "callback" => Segment::Callback(content),
            "webhook" => Segment::Webhook(content),
            "schema" => Segment::Schema(content),
            "type" => Segment::Type(content),
            "request" => Segment::RequestBody(content),
            "response" => Segment::Response(content),
            "header" => Segment::Header(content),
            "body" => Segment::Body(content),
            "param" => {
                let Some((loc, param)) = content.split_once(':') else {
                    return Err(PathParseError {
                        position: name_pos,
                        message: "parameter segment must be param[location:name]",
                    });
                };
                let Some(loc) = ParamLocation::parse(loc) else {
                    return Err(PathParseError {
                        position: name_pos,
                        message: "unknown parameter location",
                    });
                };
                Segment::Parameter(loc, param.to_owned())
            }
            _ => {
                return Err(PathParseError {
                    position: name_pos,
                    message: "unknown segment keyword",
                });
            }
        })
    }

    /// Reads bracket content up to the unescaped `]`, consuming it.
    fn bracketed(&mut self) -> Result<String, PathParseError> {
        let mut out = String::new();
        let mut chars = self.text[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                ']' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, escaped)) => out.push(escaped),
                    None => break,
                },
                c => out.push(c),
            }
        }
        self.pos = self.text.len();
        self.error("unterminated '['")
    }

    /// Reads a `"..."` string with `\"` and `\\` escapes.
    fn quoted(&mut self) -> Result<String, PathParseError> {
        self.expect("\"")?;
        let mut out = String::new();
        let mut chars = self.text[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, escaped)) => out.push(escaped),
                    None => break,
                },
                c => out.push(c),
            }
        }
        self.pos = self.text.len();
        self.error("unterminated string")
    }

    fn number(&mut self) -> Result<usize, PathParseError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.text[start..self.pos]
            .parse()
            .map_err(|_| PathParseError {
                position: start,
                message: "expected a number",
            })
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.text[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, literal: &'static str) -> Result<(), PathParseError> {
        if self.eat(literal) {
            Ok(())
        } else {
            Err(PathParseError {
                position: self.pos,
                message: match literal {
                    "[" => "expected '['",
                    "]" => "expected ']'",
                    "}" => "expected '}'",
                    "\"" => "expected '\"'",
                    _ => "unexpected character",
                },
            })
        }
    }

    fn error<T>(&self, message: &'static str) -> Result<T, PathParseError> {
        Err(PathParseError {
            position: self.pos,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(text: &str) -> IrPath {
        text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn documented_example_round_trips() {
        let text = "op[GET /pets/{}]/response[200]/body[application/json].items[].name";
        let path = p(text);
        assert_eq!(
            path.segments(),
            [
                Segment::Operation("GET /pets/{}".into()),
                Segment::Response("200".into()),
                Segment::Body("application/json".into()),
                Segment::Field("items".into()),
                Segment::Items,
                Segment::Field("name".into()),
            ]
        );
        assert_eq!(path.to_string(), text);
    }

    #[test]
    fn every_segment_kind_round_trips() {
        let path: IrPath = [
            Segment::Operation("POST /a]b\\c".into()),
            Segment::Callback("onEvent".into()),
            Segment::Webhook("newPet".into()),
            Segment::Parameter(ParamLocation::Header, "X-Id".into()),
            Segment::RequestBody("*/*".into()),
            Segment::Field("with space".into()),
            Segment::Field("quote\"d".into()),
            Segment::Arguments("pets".into()),
            Segment::Field("first".into()),
            Segment::Arguments("odd name".into()),
            Segment::PrefixItem(3),
            Segment::Contains,
            Segment::Additional,
            Segment::PatternProperty("^x-\\w+\"$".into()),
            Segment::PropertyNames,
            Segment::MapKey,
            Segment::MapValue,
            Segment::Variant(2),
            Segment::AllOf(0),
            Segment::Not,
            Segment::Type("Pet".into()),
            Segment::Schema("config".into()),
            Segment::Header("Rate".into()),
        ]
        .into_iter()
        .collect();
        let text = path.to_string();
        assert_eq!(p(&text), path, "{text}");
    }

    #[test]
    fn arguments_are_written_as_calls() {
        let path = p("type[User].pets().first");
        assert_eq!(
            path.segments(),
            [
                Segment::Type("User".into()),
                Segment::Arguments("pets".into()),
                Segment::Field("first".into())
            ]
        );
        assert_eq!(path.to_string(), "type[User].pets().first");
    }

    #[test]
    fn fields_are_quoted_only_when_needed() {
        let path: IrPath = [
            Segment::Type("T".into()),
            Segment::Field("snake_case-and-$@".into()),
            Segment::Field("1leading".into()),
            Segment::Field(String::new()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            path.to_string(),
            "type[T].snake_case-and-$@.\"1leading\".\"\""
        );
    }

    #[test]
    fn parse_errors_carry_positions() {
        let cases = [
            ("op[GET /x", 9, "unterminated '['"),
            ("nope[x]", 0, "unknown segment keyword"),
            ("param[body:x]", 0, "unknown parameter location"),
            ("type[T].", 8, "expected a field name"),
            ("type[T]|x", 8, "expected a number"),
            ("type[T]/.x", 8, "expected a keyword segment after '/'"),
            ("type[T]{~\"a", 11, "unterminated string"),
            ("type[T]#", 7, "unexpected character"),
        ];
        for (text, position, message) in cases {
            let err = text.parse::<IrPath>().unwrap_err();
            assert_eq!((err.position, err.message), (position, message), "{text}");
        }
    }

    #[test]
    fn prefix_and_parent() {
        let child = p("type[T].a.b");
        assert!(child.starts_with(&p("type[T].a")));
        assert!(!p("type[T].a").starts_with(&child));
        assert_eq!(child.parent().unwrap(), p("type[T].a"));
    }

    fn arb_segment() -> impl Strategy<Value = Segment> {
        let text = "\\PC{0,8}";
        prop_oneof![
            text.prop_map(Segment::Operation),
            text.prop_map(Segment::Body),
            (
                text,
                prop_oneof![
                    Just(ParamLocation::Path),
                    Just(ParamLocation::Query),
                    Just(ParamLocation::Header),
                    Just(ParamLocation::Cookie)
                ]
            )
                .prop_map(|(name, loc)| Segment::Parameter(loc, name)),
            text.prop_map(Segment::Field),
            text.prop_map(Segment::PatternProperty),
            any::<u16>().prop_map(|i| Segment::Variant(i.into())),
            any::<u16>().prop_map(|i| Segment::PrefixItem(i.into())),
            Just(Segment::Items),
            Just(Segment::Not),
            Just(Segment::MapValue),
        ]
    }

    proptest! {
        #[test]
        fn display_and_parse_are_inverse(segments in prop::collection::vec(arb_segment(), 0..8)) {
            let path: IrPath = segments.into_iter().collect();
            let text = path.to_string();
            prop_assert_eq!(text.parse::<IrPath>().unwrap(), path, "{}", text);
        }

        #[test]
        fn parsing_arbitrary_text_never_panics(text in "\\PC{0,32}") {
            let _ = text.parse::<IrPath>();
        }
    }
}
