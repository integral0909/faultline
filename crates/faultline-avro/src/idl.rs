//! Avro IDL (`.avdl`): protocols and schemas written as declarations.
//!
//! The IDL is read into the protocol tree an `.avpr` document would give,
//! with every node spanning the IDL it came from, so that checking and
//! lowering are shared and findings point into the IDL.
//!
//! * `protocol Name { ... }`, with `@namespace("...")`, holds named types
//!   (`record`, `error`, `enum`, `fixed`) and messages
//!   (`Type name(Type arg = default, ...) throws Error;` or `oneway`).
//! * Avro 1.12's schema form (`namespace a.b; schema T;` followed by
//!   declarations) is read as a protocol with types and no messages.
//! * A field declaration can name several fields; `T?` is a union with
//!   `null`, placed first unless the default is not null.
//! * `date`, `time_ms`, `timestamp_ms`, `local_timestamp_ms`, `uuid` and
//!   `decimal(p, s)` are logical types.
//! * `import idl`, `import protocol` and `import schema` bring in the types
//!   (and messages) of other files, resolved against the importing file.

use std::collections::HashSet;

use faultline_doc::loader::Loader;
use faultline_doc::{Decimal, Node, Number, Object, UriRef, Value};
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceId, SourceMap, Span};

use crate::codes;

/// Imports nest at most this deep; types and values nest at most
/// [`MAX_DEPTH`] deep.
const MAX_IMPORT_DEPTH: usize = 16;
const MAX_DEPTH: usize = 64;

/// Reads the IDL file at `uri` into a protocol tree.
pub fn read(
    uri: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    diagnostics: &mut Diagnostics,
) -> Option<Node> {
    let mut reader = Reader {
        loader,
        sources,
        diagnostics,
        stack: HashSet::new(),
    };
    let declarations = reader.file(uri, None, 0)?;
    Some(declarations.into_protocol())
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Number(String),
    Symbol(char),
    Error(String),
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    span: Span,
    doc: Option<String>,
}

fn tokenize(source: SourceId, text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        source,
        text,
        pos: 0,
        out: Vec::new(),
        doc: None,
    };
    lexer.run();
    lexer.out
}

struct Lexer<'t> {
    source: SourceId,
    text: &'t str,
    pos: usize,
    out: Vec<Token>,
    doc: Option<String>,
}

impl Lexer<'_> {
    fn span(&self, start: usize) -> Span {
        let offset = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Span::new(self.source, offset(start), offset(self.pos))
    }

    fn byte(&self, ahead: usize) -> Option<u8> {
        self.text.as_bytes().get(self.pos + ahead).copied()
    }

    fn push(&mut self, tok: Tok, start: usize) {
        let token = Token {
            tok,
            span: self.span(start),
            doc: self.doc.take(),
        };
        self.out.push(token);
    }

    fn run(&mut self) {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.byte(0) else {
                self.push(Tok::Eof, start);
                return;
            };
            let tok = match c {
                b'"' => self.string(),
                b'`' => self.quoted_name(),
                b'-' | b'0'..=b'9' => self.number(),
                c if c.is_ascii_alphabetic() || c == b'_' => self.word(),
                _ => {
                    let ch = self.text[self.pos..]
                        .chars()
                        .next()
                        .expect("not at the end");
                    self.pos += ch.len_utf8();
                    if "{}()[]<>,;:=@?".contains(ch) {
                        Tok::Symbol(ch)
                    } else {
                        Tok::Error(format!("unexpected {ch:?}"))
                    }
                }
            };
            self.push(tok, start);
        }
    }

    /// Skips whitespace and comments, keeping a `/** */` comment's text.
    fn skip_trivia(&mut self) {
        loop {
            match (self.byte(0), self.byte(1)) {
                (Some(c), _) if c.is_ascii_whitespace() => self.pos += 1,
                (Some(b'/'), Some(b'/')) => {
                    while self.byte(0).is_some_and(|c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                (Some(b'/'), Some(b'*')) => {
                    let start = self.pos;
                    let is_doc = self.byte(2) == Some(b'*') && self.byte(3) != Some(b'/');
                    if let Some(end) = self.text[self.pos + 2..].find("*/") {
                        let body = &self.text[self.pos + 2..self.pos + 2 + end];
                        self.pos += end + 4;
                        if is_doc {
                            self.doc = Some(doc_text(&body[1..]));
                        }
                    } else {
                        self.pos = self.text.len();
                        self.push(Tok::Error("unterminated comment".into()), start);
                    }
                }
                _ => return,
            }
        }
    }

    fn string(&mut self) -> Tok {
        self.pos += 1;
        let mut value = String::new();
        while let Some(ch) = self.text[self.pos..].chars().next() {
            self.pos += ch.len_utf8();
            match ch {
                '"' => return Tok::Str(value),
                '\\' => {
                    let Some(escaped) = self.text[self.pos..].chars().next() else {
                        break;
                    };
                    self.pos += escaped.len_utf8();
                    value.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        'b' => '\u{8}',
                        'f' => '\u{c}',
                        'u' => {
                            let hex = self.text.get(self.pos..self.pos + 4).unwrap_or_default();
                            self.pos += hex.len();
                            u32::from_str_radix(hex, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or('\u{fffd}')
                        }
                        other => other,
                    });
                }
                _ => value.push(ch),
            }
        }
        Tok::Error("unterminated string".into())
    }

    /// `` `name` ``: a name that may be a keyword.
    fn quoted_name(&mut self) -> Tok {
        self.pos += 1;
        if let Some(end) = self.text[self.pos..].find('`') {
            let name = self.text[self.pos..self.pos + end].to_owned();
            self.pos += end + 1;
            Tok::Ident(name)
        } else {
            self.pos = self.text.len();
            Tok::Error("unterminated quoted name".into())
        }
    }

    fn number(&mut self) -> Tok {
        let start = self.pos;
        self.pos += 1;
        while let Some(c) = self.byte(0) {
            let sign = matches!(c, b'+' | b'-');
            let after_exponent = matches!(self.text.as_bytes()[self.pos - 1], b'e' | b'E');
            if (c.is_ascii_alphanumeric() || c == b'.' || sign) && (!sign || after_exponent) {
                self.pos += 1;
            } else {
                break;
            }
        }
        Tok::Number(self.text[start..self.pos].to_owned())
    }

    fn word(&mut self) -> Tok {
        let start = self.pos;
        while self
            .byte(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        {
            self.pos += 1;
        }
        Tok::Ident(self.text[start..self.pos].to_owned())
    }
}

fn doc_text(body: &str) -> String {
    body.lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// What a file declares: named types in order, messages, and the protocol's
/// name and namespace.
#[derive(Default)]
struct Declarations {
    name: Option<(String, Span)>,
    namespace: Option<String>,
    doc: Option<String>,
    types: Vec<Node>,
    messages: Vec<(String, Span, Node)>,
    span: Option<Span>,
}

impl Declarations {
    fn into_protocol(self) -> Node {
        let span = self
            .span
            .unwrap_or_else(|| Span::empty(SourceId::from_raw(0), 0));
        let mut object = Object::new();
        let (name, name_span) = self.name.unwrap_or_else(|| ("Schemas".to_owned(), span));
        object.insert("protocol".into(), name_span, string(name, name_span));
        if let Some(namespace) = self.namespace {
            object.insert("namespace".into(), span, string(namespace, span));
        }
        if let Some(doc) = self.doc {
            object.insert("doc".into(), span, string(doc, span));
        }
        object.insert(
            "types".into(),
            span,
            Node::new(Value::Array(self.types), span),
        );
        let mut messages = Object::new();
        for (name, key_span, message) in self.messages {
            messages.insert(name, key_span, message);
        }
        object.insert(
            "messages".into(),
            span,
            Node::new(Value::Object(messages), span),
        );
        Node::new(Value::Object(object), span)
    }
}

fn string(text: impl Into<String>, span: Span) -> Node {
    Node::new(Value::String(text.into()), span)
}

struct Failed;
type Parsed<T> = Result<T, Failed>;

struct Reader<'a, 'd> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    diagnostics: &'d mut Diagnostics,
    /// Files being read, to stop imports that loop.
    stack: HashSet<String>,
}

impl Reader<'_, '_> {
    fn error(&mut self, message: String, span: Option<Span>) {
        let mut diagnostic = Diagnostic::error(message).with_code(codes::INVALID_SCHEMA);
        if let Some(span) = span {
            diagnostic = diagnostic.with_primary(span, "");
        }
        self.diagnostics.push(diagnostic);
    }

    /// Reads one IDL file's declarations.
    fn file(&mut self, uri: &UriRef, at: Option<Span>, depth: usize) -> Option<Declarations> {
        let key = uri.normalized().to_string();
        if depth > MAX_IMPORT_DEPTH || self.stack.contains(&key) {
            self.error(
                format!("{uri} imports itself, directly or through other files"),
                at,
            );
            return None;
        }
        let loaded = match self.loader.load(uri) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.error(format!("cannot read {uri}: {e}"), at);
                return None;
            }
        };
        let file = match self.sources.add(loaded.name, loaded.text) {
            Ok(file) => file,
            Err(e) => {
                self.error(e.to_string(), at);
                return None;
            }
        };
        self.stack.insert(key.clone());
        let mut parser = Parser {
            tokens: tokenize(file.id(), file.text()),
            pos: 0,
            messages: Vec::new(),
        };
        let parsed = parser.file();
        for (message, span) in std::mem::take(&mut parser.messages) {
            self.error(message, Some(span));
        }
        let mut declarations = Declarations {
            span: Some(file.full_span()),
            ..Declarations::default()
        };
        if let Ok(file_parsed) = parsed {
            declarations.name = file_parsed.name;
            declarations.namespace = file_parsed.namespace;
            declarations.doc = file_parsed.doc;
            for item in file_parsed.items {
                match item {
                    Item::Type(node) => declarations.types.push(node),
                    Item::Message(name, span, node) => {
                        declarations.messages.push((name, span, node));
                    }
                    Item::Import(kind, path, span) => {
                        self.import(uri, &kind, &path, span, depth, &mut declarations);
                    }
                }
            }
        }
        self.stack.remove(&key);
        Some(declarations)
    }

    fn import(
        &mut self,
        base: &UriRef,
        kind: &str,
        path: &str,
        span: Span,
        depth: usize,
        into: &mut Declarations,
    ) {
        let Ok(reference) = UriRef::parse(path) else {
            self.error(format!("{path:?} is not a valid path"), Some(span));
            return;
        };
        let target = base.resolve(&reference);
        match kind {
            "idl" => {
                if let Some(imported) = self.file(&target, Some(span), depth + 1) {
                    let namespace = imported.namespace.clone().unwrap_or_default();
                    into.types.extend(
                        imported
                            .types
                            .into_iter()
                            .map(|t| in_namespace(t, &namespace)),
                    );
                    into.messages.extend(imported.messages);
                }
            }
            "protocol" | "schema" => {
                let Some(root) = self.json(&target, span) else {
                    return;
                };
                if kind == "schema" {
                    into.types.push(root);
                    return;
                }
                let namespace = root.get("namespace").and_then(Node::as_str).unwrap_or("");
                if let Some(types) = root.get("types").and_then(Node::as_array) {
                    into.types
                        .extend(types.iter().map(|t| in_namespace(t.clone(), namespace)));
                }
                if let Some(messages) = root.get("messages").and_then(Node::as_object) {
                    for (name, member) in messages.members() {
                        into.messages.push((
                            name.to_owned(),
                            member.key_span,
                            member.value.clone(),
                        ));
                    }
                }
            }
            other => self.error(
                format!("`import {other}` is not an import; use idl, protocol or schema"),
                Some(span),
            ),
        }
    }

    fn json(&mut self, uri: &UriRef, at: Span) -> Option<Node> {
        let loaded = match self.loader.load(uri) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.error(format!("cannot read {uri}: {e}"), Some(at));
                return None;
            }
        };
        let file = self.sources.add(loaded.name, loaded.text).ok()?;
        let parsed = faultline_doc::parse(&file, faultline_doc::Format::Json, &Limits::default());
        self.diagnostics.extend(parsed.diagnostics);
        parsed.document.map(|d| d.root)
    }
}

enum Item {
    Type(Node),
    Message(String, Span, Node),
    Import(String, String, Span),
}

struct ParsedFile {
    name: Option<(String, Span)>,
    namespace: Option<String>,
    doc: Option<String>,
    items: Vec<Item>,
}

/// `@name(value)` annotations, in order.
type Annotations = Vec<(String, Span, Node)>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    messages: Vec<(String, Span)>,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn is(&self, tok: &Tok) -> bool {
        &self.peek().tok == tok
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(&self.peek().tok, Tok::Ident(w) if w == word)
    }

    fn eat(&mut self, c: char) -> bool {
        let found = self.is(&Tok::Symbol(c));
        if found {
            self.bump();
        }
        found
    }

    fn fail<T>(&mut self, expected: &str) -> Parsed<T> {
        let token = self.peek().clone();
        let message = match token.tok {
            Tok::Error(message) => message,
            Tok::Eof => format!("expected {expected}, found the end of the file"),
            Tok::Ident(w) => format!("expected {expected}, found `{w}`"),
            Tok::Str(s) => format!("expected {expected}, found {s:?}"),
            Tok::Number(n) => format!("expected {expected}, found `{n}`"),
            Tok::Symbol(c) => format!("expected {expected}, found `{c}`"),
        };
        self.messages.push((message, token.span));
        Err(Failed)
    }

    fn expect(&mut self, c: char) -> Parsed<()> {
        self.closing(c).map(|_| ())
    }

    /// Like `expect`, returning where the symbol is.
    fn closing(&mut self, c: char) -> Parsed<Span> {
        if self.is(&Tok::Symbol(c)) {
            Ok(self.bump().span)
        } else {
            self.fail(&format!("`{c}`"))
        }
    }

    fn ident(&mut self, what: &str) -> Parsed<(String, Span)> {
        match self.peek().tok.clone() {
            Tok::Ident(name) => Ok((name, self.bump().span)),
            _ => self.fail(what),
        }
    }

    fn string_literal(&mut self, what: &str) -> Parsed<(String, Span)> {
        match self.peek().tok.clone() {
            Tok::Str(s) => Ok((s, self.bump().span)),
            _ => self.fail(what),
        }
    }

    fn annotations(&mut self) -> Parsed<Annotations> {
        let mut out = Vec::new();
        while self.eat('@') {
            let (name, span) = self.ident("an annotation")?;
            self.expect('(')?;
            let value = self.json(0)?;
            self.expect(')')?;
            out.push((name, span, value));
        }
        Ok(out)
    }

    fn file(&mut self) -> Parsed<ParsedFile> {
        let doc = self.peek().doc.clone();
        let annotations = self.annotations()?;
        let mut parsed = ParsedFile {
            name: None,
            namespace: annotation_string(&annotations, "namespace"),
            doc,
            items: Vec::new(),
        };
        if self.is_word("protocol") {
            self.bump();
            parsed.name = Some(self.ident("the protocol's name")?);
            self.expect('{')?;
            while !self.eat('}') {
                if self.is(&Tok::Eof) {
                    return self.fail("`}`");
                }
                self.item(&mut parsed.items)?;
            }
            return Ok(parsed);
        }
        // The schema form: `namespace a.b;`, `schema T;` and declarations.
        while !self.is(&Tok::Eof) {
            if self.is_word("namespace") {
                self.bump();
                parsed.namespace = Some(self.ident("a namespace")?.0);
                self.expect(';')?;
            } else if self.is_word("schema") {
                self.bump();
                let schema = self.ty(0)?;
                self.expect(';')?;
                // The main schema is compared with the named types; an
                // anonymous one is a type of its own.
                if !matches!(schema.value, Value::String(_)) {
                    parsed.items.push(Item::Type(schema));
                }
            } else {
                self.item(&mut parsed.items)?;
            }
        }
        Ok(parsed)
    }

    fn item(&mut self, items: &mut Vec<Item>) -> Parsed<()> {
        if self.is_word("import") {
            self.bump();
            let (kind, _) = self.ident("idl, protocol or schema")?;
            let (path, span) = self.string_literal("the path of the file to import")?;
            self.expect(';')?;
            items.push(Item::Import(kind, path, span));
            return Ok(());
        }
        let doc = self.peek().doc.clone();
        let annotations = self.annotations()?;
        let keyword = match &self.peek().tok {
            Tok::Ident(w) => w.clone(),
            _ => return self.fail("a declaration"),
        };
        match keyword.as_str() {
            "record" | "error" => items.push(Item::Type(self.record(doc, &annotations)?)),
            "enum" => items.push(Item::Type(self.enumeration(doc, &annotations)?)),
            "fixed" => items.push(Item::Type(self.fixed(doc, &annotations)?)),
            _ => {
                let (name, span, message) = self.message(doc)?;
                items.push(Item::Message(name, span, message));
            }
        }
        Ok(())
    }

    fn named(
        &mut self,
        kind: &str,
        doc: Option<String>,
        annotations: &Annotations,
    ) -> Parsed<(Object, Span)> {
        let start = self.bump().span;
        let (name, name_span) = self.ident("a name")?;
        let mut object = Object::new();
        object.insert("type".into(), start, string(kind, start));
        object.insert("name".into(), name_span, string(name, name_span));
        if let Some(doc) = doc {
            object.insert("doc".into(), start, string(doc, start));
        }
        for (key, span, value) in annotations {
            object.insert(key.clone(), *span, value.clone());
        }
        Ok((object, start))
    }

    fn record(&mut self, doc: Option<String>, annotations: &Annotations) -> Parsed<Node> {
        let kind = if self.is_word("error") {
            "error"
        } else {
            "record"
        };
        let (mut object, start) = self.named(kind, doc, annotations)?;
        self.expect('{')?;
        let mut fields = Vec::new();
        while !self.eat('}') {
            if self.is(&Tok::Eof) {
                return self.fail("`}`");
            }
            self.fields(&mut fields)?;
        }
        let end = self.tokens[self.pos - 1].span;
        object.insert(
            "fields".into(),
            start,
            Node::new(Value::Array(fields), start),
        );
        Ok(Node::new(
            Value::Object(object),
            start.join(end).unwrap_or(start),
        ))
    }

    /// `Type name [= default], name2 [= default2];`.
    fn fields(&mut self, out: &mut Vec<Node>) -> Parsed<()> {
        let doc = self.peek().doc.clone();
        let annotations = self.annotations()?;
        let ty = self.ty(0)?;
        loop {
            let field_annotations = self.annotations()?;
            let (name, name_span) = self.ident("a field name")?;
            let mut field = Object::new();
            field.insert("name".into(), name_span, string(name, name_span));
            let default = if self.eat('=') {
                Some(self.json(0)?)
            } else {
                None
            };
            // `T?` puts `null` first unless the default is something else.
            let ty = match (&ty.value, &default) {
                (Value::Array(branches), Some(d))
                    if branches.len() == 2 && optional(&ty) && !matches!(d.value, Value::Null) =>
                {
                    Node::new(
                        Value::Array(vec![branches[1].clone(), branches[0].clone()]),
                        ty.span,
                    )
                }
                _ => ty.clone(),
            };
            field.insert("type".into(), ty.span, ty.clone());
            if let Some(default) = default {
                field.insert("default".into(), default.span, default);
            }
            if let Some(doc) = &doc {
                field.insert("doc".into(), name_span, string(doc.clone(), name_span));
            }
            for (key, span, value) in annotations.iter().chain(&field_annotations) {
                field.insert(key.clone(), *span, value.clone());
            }
            let span = ty.span.join(name_span).unwrap_or(name_span);
            out.push(Node::new(Value::Object(field), span));
            if !self.eat(',') {
                break;
            }
        }
        self.expect(';')?;
        Ok(())
    }

    fn enumeration(&mut self, doc: Option<String>, annotations: &Annotations) -> Parsed<Node> {
        let (mut object, start) = self.named("enum", doc, annotations)?;
        self.expect('{')?;
        let mut symbols = Vec::new();
        while !self.eat('}') {
            let (symbol, span) = self.ident("a symbol")?;
            symbols.push(string(symbol, span));
            if !self.eat(',') {
                self.expect('}')?;
                break;
            }
        }
        object.insert(
            "symbols".into(),
            start,
            Node::new(Value::Array(symbols), start),
        );
        if self.eat('=') {
            let (default, span) = self.ident("the default symbol")?;
            object.insert("default".into(), span, string(default, span));
        }
        self.eat(';');
        Ok(Node::new(Value::Object(object), start))
    }

    fn fixed(&mut self, doc: Option<String>, annotations: &Annotations) -> Parsed<Node> {
        let (mut object, start) = self.named("fixed", doc, annotations)?;
        self.expect('(')?;
        let size = self.json(0)?;
        self.expect(')')?;
        self.expect(';')?;
        object.insert("size".into(), size.span, size);
        Ok(Node::new(Value::Object(object), start))
    }

    /// `Type name(Type arg [= default], ...) [throws A, B | oneway];`.
    fn message(&mut self, doc: Option<String>) -> Parsed<(String, Span, Node)> {
        let response = if self.is_word("void") {
            let span = self.bump().span;
            string("null", span)
        } else {
            self.ty(0)?
        };
        let (name, name_span) = self.ident("a message name")?;
        self.expect('(')?;
        let mut request = Vec::new();
        while !self.eat(')') {
            let ty = self.ty(0)?;
            let (arg, arg_span) = self.ident("a parameter name")?;
            let mut param = Object::new();
            param.insert("name".into(), arg_span, string(arg, arg_span));
            param.insert("type".into(), ty.span, ty);
            if self.eat('=') {
                let default = self.json(0)?;
                param.insert("default".into(), default.span, default);
            }
            request.push(Node::new(Value::Object(param), arg_span));
            if !self.eat(',') {
                self.expect(')')?;
                break;
            }
        }
        let mut message = Object::new();
        if let Some(doc) = doc {
            message.insert("doc".into(), name_span, string(doc, name_span));
        }
        message.insert(
            "request".into(),
            name_span,
            Node::new(Value::Array(request), name_span),
        );
        message.insert("response".into(), response.span, response);
        if self.is_word("throws") {
            self.bump();
            let mut errors = Vec::new();
            loop {
                let (error, span) = self.ident("an error")?;
                errors.push(string(error, span));
                if !self.eat(',') {
                    break;
                }
            }
            message.insert(
                "errors".into(),
                name_span,
                Node::new(Value::Array(errors), name_span),
            );
        } else if self.is_word("oneway") {
            let span = self.bump().span;
            message.insert("one-way".into(), span, Node::new(Value::Bool(true), span));
        }
        self.expect(';')?;
        Ok((
            name.clone(),
            name_span,
            Node::new(Value::Object(message), name_span),
        ))
    }

    /// A type: a primitive, a logical type, a named reference, `array<T>`,
    /// `map<T>`, `union { ... }`, or any of these followed by `?`.
    fn ty(&mut self, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return self.fail("a less deeply nested type");
        }
        let annotations = self.annotations()?;
        let (word, span) = self.ident("a type")?;
        let mut node = match word.as_str() {
            "array" | "map" => {
                self.expect('<')?;
                let inner = self.ty(depth + 1)?;
                let end = self.closing('>')?;
                let mut object = Object::new();
                object.insert("type".into(), span, string(word.clone(), span));
                let key = if word == "array" { "items" } else { "values" };
                object.insert(key.into(), inner.span, inner);
                Node::new(Value::Object(object), span.join(end).unwrap_or(span))
            }
            "union" => {
                self.expect('{')?;
                let mut branches = Vec::new();
                while !self.eat('}') {
                    branches.push(self.ty(depth + 1)?);
                    if !self.eat(',') {
                        self.expect('}')?;
                        break;
                    }
                }
                Node::new(Value::Array(branches), span)
            }
            "decimal" => {
                self.expect('(')?;
                let precision = self.json(depth + 1)?;
                let scale = if self.eat(',') {
                    Some(self.json(depth + 1)?)
                } else {
                    None
                };
                self.expect(')')?;
                let mut object = logical("bytes", "decimal", span);
                object.insert("precision".into(), precision.span, precision);
                if let Some(scale) = scale {
                    object.insert("scale".into(), scale.span, scale);
                }
                Node::new(Value::Object(object), span)
            }
            "date" => Node::new(Value::Object(logical("int", "date", span)), span),
            "time_ms" => Node::new(Value::Object(logical("int", "time-millis", span)), span),
            "timestamp_ms" => Node::new(
                Value::Object(logical("long", "timestamp-millis", span)),
                span,
            ),
            "local_timestamp_ms" => Node::new(
                Value::Object(logical("long", "local-timestamp-millis", span)),
                span,
            ),
            "uuid" => Node::new(Value::Object(logical("string", "uuid", span)), span),
            _ => string(word, span),
        };
        if !annotations.is_empty() {
            let mut object = match node.value {
                Value::Object(object) => object,
                other => {
                    let mut object = Object::new();
                    object.insert("type".into(), span, Node::new(other, span));
                    object
                }
            };
            for (key, key_span, value) in annotations {
                object.insert(key, key_span, value);
            }
            node = Node::new(Value::Object(object), span);
        }
        if self.eat('?') {
            node = Node::new(Value::Array(vec![string("null", span), node]), span);
        }
        Ok(node)
    }

    /// A JSON value, as defaults and annotations write them.
    fn json(&mut self, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return self.fail("a less deeply nested value");
        }
        let token = self.peek().clone();
        let value = match token.tok {
            Tok::Str(s) => Value::String(s),
            Tok::Number(n) => match Decimal::parse(&n) {
                Ok(d) => Value::Number(Number::Finite(d)),
                Err(_) => return self.fail("a number"),
            },
            Tok::Ident(w) if w == "true" => Value::Bool(true),
            Tok::Ident(w) if w == "false" => Value::Bool(false),
            Tok::Ident(w) if w == "null" => Value::Null,
            Tok::Symbol('[') => {
                self.bump();
                let mut items = Vec::new();
                while !self.eat(']') {
                    items.push(self.json(depth + 1)?);
                    if !self.eat(',') {
                        self.expect(']')?;
                        break;
                    }
                }
                return Ok(Node::new(Value::Array(items), token.span));
            }
            Tok::Symbol('{') => {
                self.bump();
                let mut object = Object::new();
                while !self.eat('}') {
                    let (key, key_span) = self.string_literal("a key")?;
                    self.expect(':')?;
                    let value = self.json(depth + 1)?;
                    object.insert(key, key_span, value);
                    if !self.eat(',') {
                        self.expect('}')?;
                        break;
                    }
                }
                return Ok(Node::new(Value::Object(object), token.span));
            }
            _ => return self.fail("a value"),
        };
        self.bump();
        Ok(Node::new(value, token.span))
    }
}

/// A named type from an imported file, keeping that file's namespace
/// rather than taking the importing file's: an explicit `namespace` is
/// added where the type has none (empty for the null namespace).
fn in_namespace(mut ty: Node, namespace: &str) -> Node {
    let span = ty.span;
    if let Value::Object(object) = &mut ty.value
        && object.contains_key("name")
        && !object.contains_key("namespace")
        && !object
            .get("name")
            .and_then(Node::as_str)
            .is_some_and(|n| n.contains('.'))
    {
        object.insert("namespace".into(), span, string(namespace, span));
    }
    ty
}

/// `{"type": base, "logicalType": logical}`.
fn logical(base: &str, logical: &str, span: Span) -> Object {
    let mut object = Object::new();
    object.insert("type".into(), span, string(base, span));
    object.insert("logicalType".into(), span, string(logical, span));
    object
}

/// Whether a union came from `T?`: `null` first, one other branch.
fn optional(node: &Node) -> bool {
    matches!(&node.value, Value::Array(b) if b.first().and_then(Node::as_str) == Some("null"))
}

fn annotation_string(annotations: &Annotations, name: &str) -> Option<String> {
    annotations
        .iter()
        .find(|(key, _, _)| key == name)
        .and_then(|(_, _, value)| value.as_str())
        .map(str::to_owned)
}
