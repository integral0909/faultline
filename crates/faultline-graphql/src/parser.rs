//! Parser for GraphQL type-system documents (SDL).
//!
//! Definitions are parsed independently: a syntax error abandons the
//! current definition, the parser skips to the next token that can start
//! one, and continues. Executable definitions (operations and fragments) are
//! reported, since a schema document must not contain them, and skipped.

use faultline_source::{Diagnostic, Diagnostics, SourceId, Span};

use crate::ast::{
    Directive, DirectiveDef, Document, EnumValueDef, FieldDef, InputValue, Name, OperationKind,
    SchemaDef, TypeDef, TypeDefKind, TypeRef, Value,
};
use crate::codes;
use crate::lexer::{Token, TokenKind, tokenize};

/// Nesting of list types and constant values beyond this is reported.
const MAX_DEPTH: usize = 128;

pub fn parse(source: SourceId, text: &str) -> (Document, Diagnostics) {
    let mut parser = Parser {
        tokens: tokenize(source, text),
        pos: 0,
        depth: 0,
        diagnostics: Diagnostics::new(),
    };
    let document = parser.document();
    (document, parser.diagnostics)
}

struct Abandoned;

type PResult<T> = Result<T, Abandoned>;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    diagnostics: Diagnostics,
}

/// Keywords that start a definition, used to resynchronize after errors.
const DEFINITION_KEYWORDS: [&str; 12] = [
    "schema",
    "scalar",
    "type",
    "interface",
    "union",
    "enum",
    "input",
    "directive",
    "extend",
    "query",
    "mutation",
    "subscription",
];

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_at(&self, ahead: usize) -> &TokenKind {
        &self.tokens[(self.pos + ahead).min(self.tokens.len() - 1)].kind
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn at_punct(&self, c: char) -> bool {
        self.peek().kind == TokenKind::Punct(c)
    }

    fn at_name(&self, word: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Name(s) if s == word)
    }

    fn eat_punct(&mut self, c: char) -> bool {
        let at = self.at_punct(c);
        if at {
            self.pos += 1;
        }
        at
    }

    fn eat_name(&mut self, word: &str) -> bool {
        let at = self.at_name(word);
        if at {
            self.pos += 1;
        }
        at
    }

    fn previous_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn span_from(&self, start: Span) -> Span {
        start.join(self.previous_span()).unwrap_or(start)
    }

    fn error(&mut self, span: Span, message: impl Into<String>, label: impl Into<String>) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::SYNTAX)
                .with_primary(span, label),
        );
    }

    fn unexpected<T>(&mut self, expected: &str) -> PResult<T> {
        let token = self.peek().clone();
        let found = match &token.kind {
            TokenKind::Name(s) => format!("`{s}`"),
            TokenKind::Int(s) | TokenKind::Float(s) => format!("number {s}"),
            TokenKind::Str(_) => "a string".to_owned(),
            TokenKind::Spread => "`...`".to_owned(),
            TokenKind::Punct(c) => format!("`{c}`"),
            TokenKind::Error(message) => {
                let message = message.clone();
                self.error(token.span, message, "here");
                return Err(Abandoned);
            }
            TokenKind::Eof => "the end of the document".to_owned(),
        };
        self.error(
            token.span,
            format!("expected {expected}, found {found}"),
            format!("expected {expected}"),
        );
        Err(Abandoned)
    }

    fn expect(&mut self, c: char) -> PResult<()> {
        if self.eat_punct(c) {
            Ok(())
        } else {
            self.unexpected(&format!("`{c}`"))
        }
    }

    fn name(&mut self, what: &str) -> PResult<Name> {
        match &self.peek().kind {
            TokenKind::Name(text) => {
                let text = text.clone();
                let span = self.bump().span;
                Ok(Name { text, span })
            }
            _ => self.unexpected(what),
        }
    }

    /// Skips to the next token that can start a definition at the top
    /// level, passing over balanced braces.
    fn recover(&mut self) {
        let mut depth = 0usize;
        loop {
            let kind = self.peek().kind.clone();
            match kind {
                TokenKind::Eof => return,
                TokenKind::Punct('{') => depth += 1,
                TokenKind::Punct('}') => depth = depth.saturating_sub(1),
                _ if depth == 0 && starts_definition(&kind, self.peek_at(1)) => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    // ---- document -----------------------------------------------------

    fn document(&mut self) -> Document {
        let mut document = Document::default();
        while self.peek().kind != TokenKind::Eof {
            let start = self.pos;
            if let Err(Abandoned) = self.definition(&mut document) {
                if self.pos == start {
                    self.pos += 1;
                }
                self.recover();
            }
        }
        document
    }

    fn definition(&mut self, document: &mut Document) -> PResult<()> {
        let start = self.peek().span;
        let description = self.description();
        if self.at_punct('{') {
            return self.executable(start);
        }
        let TokenKind::Name(keyword) = self.peek().kind.clone() else {
            return self.unexpected("a definition");
        };
        match keyword.as_str() {
            "query" | "mutation" | "subscription" | "fragment" => self.executable(start),
            "schema" => {
                let schema = self.schema(description, false, start)?;
                document.schemas.push(schema);
                Ok(())
            }
            "directive" => {
                let directive = self.directive_def(description, start)?;
                document.directives.push(directive);
                Ok(())
            }
            "extend" => {
                if description.is_some() {
                    self.error(
                        start,
                        "extensions cannot have descriptions",
                        "remove this description",
                    );
                }
                self.bump();
                if self.at_name("schema") {
                    let schema = self.schema(None, true, start)?;
                    document.schemas.push(schema);
                } else {
                    let ty = self.type_def(None, true, start)?;
                    document.types.push(ty);
                }
                Ok(())
            }
            _ => {
                let ty = self.type_def(description, false, start)?;
                document.types.push(ty);
                Ok(())
            }
        }
    }

    fn description(&mut self) -> Option<String> {
        match &self.peek().kind {
            TokenKind::Str(s) => {
                let s = s.clone();
                self.bump();
                Some(s)
            }
            _ => None,
        }
    }

    /// Reports and skips a query, mutation, subscription or fragment.
    fn executable(&mut self, start: Span) -> PResult<()> {
        self.diagnostics.push(
            Diagnostic::error("a schema document cannot contain operations or fragments")
                .with_code(codes::EXECUTABLE)
                .with_primary(start, "executable definition"),
        );
        // Skip to the end of the selection set.
        while !self.at_punct('{') && self.peek().kind != TokenKind::Eof {
            self.bump();
        }
        let mut depth = 0usize;
        loop {
            match self.bump().kind {
                TokenKind::Punct('{') => depth += 1,
                TokenKind::Punct('}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(());
                    }
                }
                TokenKind::Eof => return Ok(()),
                _ => {}
            }
        }
    }

    fn schema(
        &mut self,
        description: Option<String>,
        extension: bool,
        start: Span,
    ) -> PResult<SchemaDef> {
        self.bump();
        let directives = self.directives()?;
        let mut roots = Vec::new();
        if self.eat_punct('{') {
            while !self.eat_punct('}') {
                let keyword = self.name("`query`, `mutation` or `subscription`")?;
                let Some(kind) = OperationKind::parse(&keyword.text) else {
                    self.error(
                        keyword.span,
                        format!("unknown operation type `{}`", keyword.text),
                        "expected `query`, `mutation` or `subscription`",
                    );
                    return Err(Abandoned);
                };
                self.expect(':')?;
                let name = self.name("a type name")?;
                roots.push((kind, name));
            }
        } else if !extension {
            return self.unexpected("`{`");
        }
        Ok(SchemaDef {
            description,
            roots,
            directives,
            extension,
            span: self.span_from(start),
        })
    }

    fn directive_def(&mut self, description: Option<String>, start: Span) -> PResult<DirectiveDef> {
        self.bump();
        self.expect('@')?;
        let name = self.name("a directive name")?;
        let arguments = self.arguments_def()?;
        let repeatable = self.eat_name("repeatable");
        if !self.eat_name("on") {
            return self.unexpected("`on`");
        }
        self.eat_punct('|');
        let mut locations = vec![self.name("a directive location")?];
        while self.eat_punct('|') {
            locations.push(self.name("a directive location")?);
        }
        Ok(DirectiveDef {
            description,
            name,
            arguments,
            repeatable,
            locations,
            span: self.span_from(start),
        })
    }

    fn type_def(
        &mut self,
        description: Option<String>,
        extension: bool,
        start: Span,
    ) -> PResult<TypeDef> {
        let keyword = self.name("a type definition")?;
        let name = self.name("a type name")?;
        let kind = match keyword.text.as_str() {
            "scalar" => TypeDefKind::Scalar,
            "type" | "interface" => {
                let interfaces = self.implements()?;
                let kind_directives = self.directives()?;
                let fields = if self.at_punct('{') {
                    self.fields_def()?
                } else {
                    Vec::new()
                };
                let kind = if keyword.text == "type" {
                    TypeDefKind::Object { interfaces, fields }
                } else {
                    TypeDefKind::Interface { interfaces, fields }
                };
                return Ok(TypeDef {
                    description,
                    name,
                    kind,
                    directives: kind_directives,
                    extension,
                    span: self.span_from(start),
                });
            }
            "union" => {
                let directives = self.directives()?;
                let mut members = Vec::new();
                if self.eat_punct('=') {
                    self.eat_punct('|');
                    members.push(self.name("a member type")?);
                    while self.eat_punct('|') {
                        members.push(self.name("a member type")?);
                    }
                }
                return Ok(TypeDef {
                    description,
                    name,
                    kind: TypeDefKind::Union { members },
                    directives,
                    extension,
                    span: self.span_from(start),
                });
            }
            "enum" => {
                let directives = self.directives()?;
                let values = if self.at_punct('{') {
                    self.enum_values()?
                } else {
                    Vec::new()
                };
                return Ok(TypeDef {
                    description,
                    name,
                    kind: TypeDefKind::Enum { values },
                    directives,
                    extension,
                    span: self.span_from(start),
                });
            }
            "input" => {
                let directives = self.directives()?;
                let fields = if self.at_punct('{') {
                    self.input_fields()?
                } else {
                    Vec::new()
                };
                return Ok(TypeDef {
                    description,
                    name,
                    kind: TypeDefKind::InputObject { fields },
                    directives,
                    extension,
                    span: self.span_from(start),
                });
            }
            other => {
                self.error(
                    keyword.span,
                    format!("unknown definition `{other}`"),
                    "expected `type`, `interface`, `union`, `enum`, `input` or `scalar`",
                );
                return Err(Abandoned);
            }
        };
        let directives = self.directives()?;
        Ok(TypeDef {
            description,
            name,
            kind,
            directives,
            extension,
            span: self.span_from(start),
        })
    }

    fn implements(&mut self) -> PResult<Vec<Name>> {
        let mut interfaces = Vec::new();
        if self.eat_name("implements") {
            self.eat_punct('&');
            interfaces.push(self.name("an interface name")?);
            while self.eat_punct('&') {
                interfaces.push(self.name("an interface name")?);
            }
            // The legacy form separated interfaces by spaces.
            while let TokenKind::Name(n) = &self.peek().kind {
                if self.peek_at(1) == &TokenKind::Punct(':') || n == "extend" {
                    break;
                }
                interfaces.push(self.name("an interface name")?);
            }
        }
        Ok(interfaces)
    }

    fn fields_def(&mut self) -> PResult<Vec<FieldDef>> {
        self.expect('{')?;
        let mut fields = Vec::new();
        while !self.eat_punct('}') {
            let start = self.peek().span;
            let description = self.description();
            let name = self.name("a field name")?;
            let arguments = self.arguments_def()?;
            self.expect(':')?;
            let ty = self.type_ref()?;
            let directives = self.directives()?;
            fields.push(FieldDef {
                description,
                name,
                arguments,
                ty,
                directives,
                span: self.span_from(start),
            });
        }
        Ok(fields)
    }

    fn arguments_def(&mut self) -> PResult<Vec<InputValue>> {
        let mut arguments = Vec::new();
        if self.eat_punct('(') {
            while !self.eat_punct(')') {
                arguments.push(self.input_value()?);
            }
        }
        Ok(arguments)
    }

    fn input_fields(&mut self) -> PResult<Vec<InputValue>> {
        self.expect('{')?;
        let mut fields = Vec::new();
        while !self.eat_punct('}') {
            fields.push(self.input_value()?);
        }
        Ok(fields)
    }

    fn input_value(&mut self) -> PResult<InputValue> {
        let start = self.peek().span;
        let description = self.description();
        let name = self.name("an argument name")?;
        self.expect(':')?;
        let ty = self.type_ref()?;
        let default = if self.eat_punct('=') {
            Some(self.value()?)
        } else {
            None
        };
        let directives = self.directives()?;
        Ok(InputValue {
            description,
            name,
            ty,
            default,
            directives,
            span: self.span_from(start),
        })
    }

    fn enum_values(&mut self) -> PResult<Vec<EnumValueDef>> {
        self.expect('{')?;
        let mut values = Vec::new();
        while !self.eat_punct('}') {
            let start = self.peek().span;
            let description = self.description();
            let name = self.name("an enum value")?;
            if matches!(name.text.as_str(), "true" | "false" | "null") {
                self.error(
                    name.span,
                    format!("`{}` cannot be an enum value", name.text),
                    "reserved word",
                );
            }
            let directives = self.directives()?;
            values.push(EnumValueDef {
                description,
                name,
                directives,
                span: self.span_from(start),
            });
        }
        Ok(values)
    }

    fn type_ref(&mut self) -> PResult<TypeRef> {
        let start = self.peek().span;
        let ty = if self.eat_punct('[') {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                self.error(start, "type is nested too deeply", "here");
                return Err(Abandoned);
            }
            let inner = self.type_ref();
            self.depth -= 1;
            let inner = inner?;
            self.expect(']')?;
            TypeRef::List(Box::new(inner), self.span_from(start))
        } else {
            TypeRef::Named(self.name("a type")?)
        };
        if self.eat_punct('!') {
            Ok(TypeRef::NonNull(Box::new(ty), self.span_from(start)))
        } else {
            Ok(ty)
        }
    }

    fn directives(&mut self) -> PResult<Vec<Directive>> {
        let mut directives = Vec::new();
        while self.at_punct('@') {
            let start = self.bump().span;
            let name = self.name("a directive name")?;
            let mut arguments = Vec::new();
            if self.eat_punct('(') {
                while !self.eat_punct(')') {
                    let arg = self.name("an argument name")?;
                    self.expect(':')?;
                    arguments.push((arg, self.value()?));
                }
            }
            directives.push(Directive {
                name,
                arguments,
                span: self.span_from(start),
            });
        }
        Ok(directives)
    }

    fn value(&mut self) -> PResult<Value> {
        self.depth += 1;
        let result = if self.depth > MAX_DEPTH {
            let span = self.peek().span;
            self.error(span, "value is nested too deeply", "here");
            Err(Abandoned)
        } else {
            self.value_inner()
        };
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self) -> PResult<Value> {
        let token = self.peek().clone();
        let value = match token.kind {
            TokenKind::Int(s) => Value::Int(s),
            TokenKind::Float(s) => Value::Float(s),
            TokenKind::Str(s) => Value::String(s),
            TokenKind::Name(s) => match s.as_str() {
                "true" => Value::Boolean(true),
                "false" => Value::Boolean(false),
                "null" => Value::Null,
                _ => Value::Enum(s),
            },
            TokenKind::Punct('$') => {
                self.bump();
                let name = self.name("a variable name")?;
                self.error(
                    self.span_from(token.span),
                    "variables are not allowed in a schema",
                    "use a constant",
                );
                return Ok(Value::Variable(name.text));
            }
            TokenKind::Punct('[') => {
                self.bump();
                let mut items = Vec::new();
                while !self.eat_punct(']') {
                    items.push(self.value()?);
                }
                return Ok(Value::List(items));
            }
            TokenKind::Punct('{') => {
                self.bump();
                let mut fields = Vec::new();
                while !self.eat_punct('}') {
                    let name = self.name("a field name")?;
                    self.expect(':')?;
                    fields.push((name.text, self.value()?));
                }
                return Ok(Value::Object(fields));
            }
            _ => return self.unexpected("a value"),
        };
        self.bump();
        Ok(value)
    }
}

/// Whether `token` (followed by `next`) can start a definition.
fn starts_definition(token: &TokenKind, next: &TokenKind) -> bool {
    match token {
        TokenKind::Str(_) => {
            matches!(next, TokenKind::Name(n) if DEFINITION_KEYWORDS.contains(&n.as_str()))
        }
        TokenKind::Name(n) => {
            DEFINITION_KEYWORDS.contains(&n.as_str())
                && matches!(next, TokenKind::Name(_) | TokenKind::Punct('{' | '@'))
        }
        _ => false,
    }
}
