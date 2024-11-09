//! Parsing Thrift IDL into a [`Document`].
//!
//! The parser reports the first problem in a definition and resumes at the
//! next top-level keyword, so one mistake does not hide the rest of the
//! file's problems.

use faultline_source::{Diagnostic, Diagnostics, SourceId, Span};

use crate::ast::{
    Annotations, BaseType, Const, ConstValue, Definition, Document, Enum, EnumValue, Field,
    FieldType, FieldTypeKind, Function, Include, Name, Requiredness, Service, Struct, StructKind,
    Typedef,
};
use crate::codes;
use crate::lexer::{Token, TokenKind, tokenize};

/// Types and constant values nest at most this deep.
const MAX_DEPTH: usize = 64;

/// The words that start a header or a definition.
const TOP_LEVEL: [&str; 12] = [
    "include",
    "cpp_include",
    "namespace",
    "const",
    "typedef",
    "enum",
    "senum",
    "struct",
    "union",
    "exception",
    "service",
    "php_namespace",
];

/// Parses `text`. The document holds every definition that parsed.
pub fn parse(source: SourceId, text: &str) -> (Document, Diagnostics) {
    let mut parser = Parser {
        tokens: tokenize(source, text),
        pos: 0,
        diagnostics: Diagnostics::new(),
    };
    let document = parser.document();
    (document, parser.diagnostics)
}

/// A syntax error, already reported.
struct Failed;

type Parsed<T> = Result<T, Failed>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Diagnostics,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_at(&self, ahead: usize) -> &Token {
        &self.tokens[(self.pos + ahead).min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(w) if w == word)
    }

    fn is_symbol(&self, symbol: char) -> bool {
        self.peek().kind == TokenKind::Symbol(symbol)
    }

    fn eat_symbol(&mut self, symbol: char) -> bool {
        let found = self.is_symbol(symbol);
        if found {
            self.bump();
        }
        found
    }

    fn eat_word(&mut self, word: &str) -> bool {
        let found = self.is_word(word);
        if found {
            self.bump();
        }
        found
    }

    fn error<T>(&mut self, expected: &str) -> Parsed<T> {
        let token = self.peek();
        let message = match &token.kind {
            TokenKind::Error(message) => message.clone(),
            TokenKind::Eof => format!("expected {expected}, found the end of the file"),
            kind => format!("expected {expected}, found {}", describe(kind)),
        };
        let span = token.span;
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::SYNTAX)
                .with_primary(span, ""),
        );
        Err(Failed)
    }

    fn warn(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::warning(message)
                .with_code(codes::DEPRECATED_SYNTAX)
                .with_primary(span, ""),
        );
    }

    fn expect_symbol(&mut self, symbol: char) -> Parsed<()> {
        if self.eat_symbol(symbol) {
            Ok(())
        } else {
            self.error(&format!("`{symbol}`"))
        }
    }

    fn name(&mut self, what: &str) -> Parsed<Name> {
        match &self.peek().kind {
            TokenKind::Ident(text) => {
                let name = Name {
                    text: text.clone(),
                    span: self.peek().span,
                };
                self.bump();
                Ok(name)
            }
            _ => self.error(what),
        }
    }

    fn string(&mut self, what: &str) -> Parsed<(String, Span)> {
        match &self.peek().kind {
            TokenKind::Str(text) => {
                let result = (text.clone(), self.peek().span);
                self.bump();
                Ok(result)
            }
            _ => self.error(what),
        }
    }

    /// An optional `,` or `;` between list items.
    fn separator(&mut self) {
        if !self.eat_symbol(',') {
            self.eat_symbol(';');
        }
    }

    fn document(&mut self) -> Document {
        let mut document = Document::default();
        while self.peek().kind != TokenKind::Eof {
            let start = self.pos;
            if self.top_level(&mut document).is_err() {
                self.recover(start);
            }
        }
        document
    }

    /// Skips to the next top-level keyword after a syntax error.
    fn recover(&mut self, start: usize) {
        if self.pos == start {
            self.bump();
        }
        let mut depth = 0usize;
        loop {
            match &self.peek().kind {
                TokenKind::Eof => return,
                TokenKind::Symbol('{') => depth += 1,
                TokenKind::Symbol('}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                TokenKind::Ident(word) if depth == 0 && TOP_LEVEL.contains(&word.as_str()) => {
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    fn top_level(&mut self, document: &mut Document) -> Parsed<()> {
        let doc = self.peek().doc.clone();
        let TokenKind::Ident(word) = self.peek().kind.clone() else {
            return self.error("a definition");
        };
        match word.as_str() {
            "include" => {
                self.bump();
                let (path, span) = self.string("the path of the file to include")?;
                document.includes.push(Include { path, span });
            }
            "cpp_include" => {
                self.bump();
                let _ = self.string("a C++ include")?;
            }
            "namespace" => {
                self.bump();
                let scope = if self.eat_symbol('*') {
                    "*".to_owned()
                } else {
                    self.name("a namespace scope")?.text
                };
                let name = self.name("a namespace")?;
                self.annotations()?;
                document.namespaces.push((scope, name));
            }
            "php_namespace" | "xsd_namespace" | "smalltalk.category" | "smalltalk.prefix" => {
                let span = self.bump().span;
                let _ = self.string("a namespace")?;
                self.warn(format!("`{word}` is deprecated and ignored"), span);
            }
            "const" => {
                self.bump();
                let ty = self.field_type(0)?;
                let name = self.name("the constant's name")?;
                self.expect_symbol('=')?;
                let value = self.const_value(0)?;
                self.separator();
                document.definitions.push(Definition::Const(Const {
                    name,
                    ty,
                    value,
                    doc,
                }));
            }
            "typedef" => {
                self.bump();
                let ty = self.field_type(0)?;
                let name = self.name("the typedef's name")?;
                let annotations = self.annotations()?;
                self.separator();
                document.definitions.push(Definition::Typedef(Typedef {
                    name,
                    ty,
                    doc,
                    annotations,
                }));
            }
            "enum" => {
                self.bump();
                let definition = self.enumeration(doc)?;
                document.definitions.push(Definition::Enum(definition));
            }
            "senum" => {
                let span = self.bump().span;
                self.warn("`senum` is deprecated and ignored".into(), span);
                let _ = self.name("the senum's name")?;
                self.expect_symbol('{')?;
                while !self.eat_symbol('}') {
                    let _ = self.string("a string")?;
                    self.separator();
                }
            }
            "struct" | "union" | "exception" => {
                self.bump();
                let kind = match word.as_str() {
                    "struct" => StructKind::Struct,
                    "union" => StructKind::Union,
                    _ => StructKind::Exception,
                };
                let definition = self.structure(kind, doc)?;
                document.definitions.push(Definition::Struct(definition));
            }
            "service" => {
                self.bump();
                let definition = self.service(doc)?;
                document.definitions.push(Definition::Service(definition));
            }
            _ => return self.error("a definition"),
        }
        Ok(())
    }

    fn enumeration(&mut self, doc: Option<String>) -> Parsed<Enum> {
        let name = self.name("the enum's name")?;
        self.expect_symbol('{')?;
        let mut values = Vec::new();
        while !self.eat_symbol('}') {
            let doc = self.peek().doc.clone();
            let value_name = self.name("an enum value or `}`")?;
            let value = if self.eat_symbol('=') {
                match self.peek().kind {
                    TokenKind::Int(n) => Some((n, self.bump().span)),
                    _ => return self.error("an integer"),
                }
            } else {
                None
            };
            let annotations = self.annotations()?;
            self.separator();
            values.push(EnumValue {
                name: value_name,
                value,
                doc,
                annotations,
            });
        }
        let annotations = self.annotations()?;
        Ok(Enum {
            name,
            values,
            doc,
            annotations,
        })
    }

    fn structure(&mut self, kind: StructKind, doc: Option<String>) -> Parsed<Struct> {
        let name = self.name(&format!("the {}'s name", kind.keyword()))?;
        self.eat_word("xsd_all");
        self.expect_symbol('{')?;
        let fields = self.fields('}')?;
        let annotations = self.annotations()?;
        Ok(Struct {
            kind,
            name,
            fields,
            doc,
            annotations,
        })
    }

    /// Fields up to and including `close`.
    fn fields(&mut self, close: char) -> Parsed<Vec<Field>> {
        let mut fields = Vec::new();
        while !self.eat_symbol(close) {
            fields.push(self.field()?);
        }
        Ok(fields)
    }

    fn field(&mut self) -> Parsed<Field> {
        let doc = self.peek().doc.clone();
        let start = self.peek().span;
        let id = match self.peek().kind {
            TokenKind::Int(n) if self.peek_at(1).kind == TokenKind::Symbol(':') => {
                let span = self.bump().span;
                self.bump();
                Some((n, span))
            }
            _ => None,
        };
        let requiredness = if self.eat_word("required") {
            Requiredness::Required
        } else if self.eat_word("optional") {
            Requiredness::Optional
        } else {
            Requiredness::Default
        };
        let ty = self.field_type(0)?;
        let name = self.name("the field's name")?;
        let default = if self.eat_symbol('=') {
            Some(self.const_value(0)?)
        } else {
            None
        };
        // XSD options, which only affect XML generation.
        self.eat_word("xsd_optional");
        self.eat_word("xsd_nillable");
        let annotations = self.annotations()?;
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        self.separator();
        Ok(Field {
            id,
            requiredness,
            ty,
            name,
            default,
            doc,
            annotations,
            span: start.join(end).unwrap_or(start),
        })
    }

    fn field_type(&mut self, depth: usize) -> Parsed<FieldType> {
        if depth > MAX_DEPTH {
            return self.error("a less deeply nested type");
        }
        let start = self.peek().span;
        let TokenKind::Ident(word) = self.peek().kind.clone() else {
            return self.error("a type");
        };
        self.bump();
        let kind = match word.as_str() {
            "slist" => {
                self.warn("`slist` is deprecated; read as `string`".into(), start);
                FieldTypeKind::Base(BaseType::String)
            }
            "list" | "set" => {
                self.cpp_type()?;
                self.expect_symbol('<')?;
                let item = self.field_type(depth + 1)?;
                self.expect_symbol('>')?;
                self.cpp_type()?;
                if word == "list" {
                    FieldTypeKind::List(Box::new(item))
                } else {
                    FieldTypeKind::Set(Box::new(item))
                }
            }
            "map" => {
                self.cpp_type()?;
                self.expect_symbol('<')?;
                let key = self.field_type(depth + 1)?;
                self.expect_symbol(',')?;
                let value = self.field_type(depth + 1)?;
                self.expect_symbol('>')?;
                FieldTypeKind::Map(Box::new(key), Box::new(value))
            }
            _ => match BaseType::parse(&word) {
                Some(base) => FieldTypeKind::Base(base),
                None => FieldTypeKind::Named(word),
            },
        };
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        let annotations = self.annotations()?;
        Ok(FieldType {
            kind,
            span: start.join(end).unwrap_or(start),
            annotations,
        })
    }

    /// An optional `cpp_type "..."`, which only affects generated C++.
    fn cpp_type(&mut self) -> Parsed<()> {
        if self.eat_word("cpp_type") {
            let _ = self.string("a C++ type")?;
        }
        Ok(())
    }

    fn const_value(&mut self, depth: usize) -> Parsed<ConstValue> {
        if depth > MAX_DEPTH {
            return self.error("a less deeply nested value");
        }
        let value = match self.peek().kind.clone() {
            TokenKind::Int(n) => ConstValue::Int(n),
            TokenKind::Double(d) => ConstValue::Double(d),
            TokenKind::Str(s) => ConstValue::Str(s),
            TokenKind::Ident(name) => ConstValue::Ident(name),
            TokenKind::Symbol('[') => {
                self.bump();
                let mut items = Vec::new();
                while !self.eat_symbol(']') {
                    items.push(self.const_value(depth + 1)?);
                    self.separator();
                }
                return Ok(ConstValue::List(items));
            }
            TokenKind::Symbol('{') => {
                self.bump();
                let mut entries = Vec::new();
                while !self.eat_symbol('}') {
                    let key = self.const_value(depth + 1)?;
                    self.expect_symbol(':')?;
                    let value = self.const_value(depth + 1)?;
                    self.separator();
                    entries.push((key, value));
                }
                return Ok(ConstValue::Map(entries));
            }
            _ => return self.error("a constant value"),
        };
        self.bump();
        Ok(value)
    }

    /// `( key = "value", key2, ... )`, if present.
    fn annotations(&mut self) -> Parsed<Annotations> {
        let mut annotations = Vec::new();
        if !self.eat_symbol('(') {
            return Ok(annotations);
        }
        while !self.eat_symbol(')') {
            let key = self.name("an annotation")?;
            let value = if self.eat_symbol('=') {
                Some(self.string("the annotation's value")?.0)
            } else {
                None
            };
            self.separator();
            annotations.push((key, value));
        }
        Ok(annotations)
    }

    fn service(&mut self, doc: Option<String>) -> Parsed<Service> {
        let name = self.name("the service's name")?;
        let extends = if self.eat_word("extends") {
            Some(self.name("the service it extends")?)
        } else {
            None
        };
        self.expect_symbol('{')?;
        let mut functions = Vec::new();
        while !self.eat_symbol('}') {
            functions.push(self.function()?);
        }
        let annotations = self.annotations()?;
        Ok(Service {
            name,
            extends,
            functions,
            doc,
            annotations,
        })
    }

    fn function(&mut self) -> Parsed<Function> {
        let doc = self.peek().doc.clone();
        let oneway = if self.eat_word("oneway") {
            true
        } else {
            if self.is_word("async") {
                let span = self.bump().span;
                self.warn("`async` is deprecated; read as `oneway`".into(), span);
                return self.function_rest(doc, true);
            }
            false
        };
        self.function_rest(doc, oneway)
    }

    fn function_rest(&mut self, doc: Option<String>, oneway: bool) -> Parsed<Function> {
        let return_span = self.peek().span;
        let returns = if self.eat_word("void") {
            None
        } else {
            Some(self.field_type(0)?)
        };
        let name = self.name("the function's name")?;
        self.expect_symbol('(')?;
        let arguments = self.fields(')')?;
        let throws = if self.eat_word("throws") {
            self.expect_symbol('(')?;
            self.fields(')')?
        } else {
            Vec::new()
        };
        let annotations = self.annotations()?;
        self.separator();
        Ok(Function {
            name,
            oneway,
            returns,
            return_span,
            arguments,
            throws,
            doc,
            annotations,
        })
    }
}

fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(word) => format!("`{word}`"),
        TokenKind::Int(n) => format!("`{n}`"),
        TokenKind::Double(d) => format!("`{d}`"),
        TokenKind::Str(s) => format!("{s:?}"),
        TokenKind::Symbol(c) => format!("`{c}`"),
        TokenKind::Error(message) => message.clone(),
        TokenKind::Eof => "the end of the file".to_owned(),
    }
}
