//! Parsing Smithy IDL 2.0 into a [`Model`].
//!
//! After a syntax error the parser resumes at the next shape, trait or
//! `apply` statement, so one mistake does not hide the rest of the file's
//! problems.

use faultline_source::{Diagnostic, Diagnostics, SourceId};

use crate::ast::{
    Apply, IoShape, Member, Model, Name, Node, NodeValue, OperationBody, Shape, ShapeKind,
    SimpleType, Trait,
};
use crate::codes;
use crate::lexer::{Token, TokenKind, tokenize};

/// Node values nest at most this deep.
const MAX_DEPTH: usize = 64;

/// Parses `text`. The model holds every statement that parsed.
pub fn parse(source: SourceId, text: &str) -> (Model, Diagnostics) {
    let mut parser = Parser {
        tokens: tokenize(source, text),
        pos: 0,
        diagnostics: Diagnostics::new(),
    };
    let model = parser.model();
    (model, parser.diagnostics)
}

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

    fn is_symbol(&self, c: char) -> bool {
        self.peek().kind == TokenKind::Symbol(c)
    }

    fn eat_symbol(&mut self, c: char) -> bool {
        let found = self.is_symbol(c);
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
            TokenKind::Ident(w) => format!("expected {expected}, found `{w}`"),
            TokenKind::Number(n) => format!("expected {expected}, found `{n}`"),
            TokenKind::Str(s) => format!("expected {expected}, found {s:?}"),
            TokenKind::Symbol(c) => format!("expected {expected}, found `{c}`"),
            TokenKind::Walrus => format!("expected {expected}, found `:=`"),
        };
        let span = token.span;
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::SYNTAX)
                .with_primary(span, ""),
        );
        Err(Failed)
    }

    fn expect_symbol(&mut self, c: char) -> Parsed<()> {
        if self.eat_symbol(c) {
            Ok(())
        } else {
            self.error(&format!("`{c}`"))
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

    fn model(&mut self) -> Model {
        let mut model = Model::default();
        while self.peek().kind != TokenKind::Eof {
            let start = self.pos;
            if self.statement(&mut model).is_err() {
                self.recover(start);
            }
        }
        model
    }

    /// Skips to the next statement after a syntax error.
    fn recover(&mut self, start: usize) {
        if self.pos == start {
            self.bump();
        }
        let mut depth = 0usize;
        loop {
            match &self.peek().kind {
                TokenKind::Eof => return,
                TokenKind::Symbol('{' | '[' | '(') => depth += 1,
                TokenKind::Symbol('}' | ']' | ')') => depth = depth.saturating_sub(1),
                TokenKind::Symbol('@') if depth == 0 => return,
                TokenKind::Ident(w) if depth == 0 && starts_shape(w, &self.peek_at(1).kind) => {
                    return;
                }
                _ => {}
            }
            self.bump();
        }
    }

    fn statement(&mut self, model: &mut Model) -> Parsed<()> {
        if self.is_symbol('$') {
            self.bump();
            let key = self.name("a control statement")?;
            self.expect_symbol(':')?;
            let value = self.node(0)?;
            if key.text == "version" {
                model.version = value.as_str().map(str::to_owned);
            }
            return Ok(());
        }
        if self.is_word("metadata") {
            self.bump();
            let _ = self.node_key()?;
            self.expect_symbol('=')?;
            self.node(0)?;
            return Ok(());
        }
        if self.is_word("namespace") && matches!(self.peek_at(1).kind, TokenKind::Ident(_)) {
            self.bump();
            model.namespace = Some(self.name("a namespace")?);
            return Ok(());
        }
        if self.is_word("use") && matches!(self.peek_at(1).kind, TokenKind::Ident(_)) {
            self.bump();
            model.uses.push(self.name("a shape id")?);
            return Ok(());
        }
        if self.is_word("apply") && matches!(self.peek_at(1).kind, TokenKind::Ident(_)) {
            self.bump();
            let target = self.name("a shape id")?;
            let traits = if self.eat_symbol('{') {
                let mut traits = Vec::new();
                while !self.eat_symbol('}') {
                    traits.push(self.one_trait()?);
                }
                traits
            } else {
                vec![self.one_trait()?]
            };
            model.applies.push(Apply { target, traits });
            return Ok(());
        }
        let doc = self.peek().doc.clone();
        let traits = self.traits()?;
        let shape = self.shape(doc, traits)?;
        model.shapes.push(shape);
        Ok(())
    }

    fn traits(&mut self) -> Parsed<Vec<Trait>> {
        let mut traits = Vec::new();
        while self.is_symbol('@') {
            traits.push(self.one_trait()?);
        }
        Ok(traits)
    }

    /// `@id`, `@id(value)` or `@id(key: value, ...)`.
    fn one_trait(&mut self) -> Parsed<Trait> {
        let at = self.peek().span;
        self.expect_symbol('@')?;
        let id = self.name("a trait")?;
        if !self.is_symbol('(') {
            return Ok(Trait {
                value: Node {
                    value: NodeValue::Null,
                    span: at.join(id.span).unwrap_or(id.span),
                },
                id,
            });
        }
        let open = self.bump().span;
        if self.is_symbol(')') {
            let close = self.bump().span;
            return Ok(Trait {
                id,
                value: Node {
                    value: NodeValue::Object(Vec::new()),
                    span: open.join(close).unwrap_or(open),
                },
            });
        }
        // `key: value` pairs make a structure; anything else is one value.
        let structured = matches!(self.peek().kind, TokenKind::Ident(_) | TokenKind::Str(_))
            && self.peek_at(1).kind == TokenKind::Symbol(':');
        let value = if structured {
            let mut members = Vec::new();
            while !self.is_symbol(')') {
                let key = self.node_key()?;
                self.expect_symbol(':')?;
                members.push((key, self.node(1)?));
            }
            let close = self.peek().span;
            Node {
                value: NodeValue::Object(members),
                span: open.join(close).unwrap_or(open),
            }
        } else {
            self.node(1)?
        };
        self.expect_symbol(')')?;
        Ok(Trait { id, value })
    }

    fn node_key(&mut self) -> Parsed<Name> {
        match &self.peek().kind {
            TokenKind::Ident(text) | TokenKind::Str(text) => {
                let name = Name {
                    text: text.clone(),
                    span: self.peek().span,
                };
                self.bump();
                Ok(name)
            }
            _ => self.error("a key"),
        }
    }

    fn node(&mut self, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return self.error("a less deeply nested value");
        }
        let token = self.peek().clone();
        let value = match token.kind {
            TokenKind::Str(s) => NodeValue::String(s),
            TokenKind::Number(n) => NodeValue::Number(n),
            TokenKind::Ident(w) => match w.as_str() {
                "true" => NodeValue::Bool(true),
                "false" => NodeValue::Bool(false),
                "null" => NodeValue::Null,
                _ => NodeValue::ShapeId(w),
            },
            TokenKind::Symbol('[') => {
                self.bump();
                let mut items = Vec::new();
                while !self.is_symbol(']') {
                    items.push(self.node(depth + 1)?);
                }
                let end = self.bump().span;
                return Ok(Node {
                    value: NodeValue::Array(items),
                    span: token.span.join(end).unwrap_or(token.span),
                });
            }
            TokenKind::Symbol('{') => {
                self.bump();
                let mut members = Vec::new();
                while !self.is_symbol('}') {
                    let key = self.node_key()?;
                    self.expect_symbol(':')?;
                    members.push((key, self.node(depth + 1)?));
                }
                let end = self.bump().span;
                return Ok(Node {
                    value: NodeValue::Object(members),
                    span: token.span.join(end).unwrap_or(token.span),
                });
            }
            _ => return self.error("a value"),
        };
        self.bump();
        Ok(Node {
            value,
            span: token.span,
        })
    }

    fn shape(&mut self, doc: Option<String>, traits: Vec<Trait>) -> Parsed<Shape> {
        let TokenKind::Ident(keyword) = self.peek().kind.clone() else {
            return self.error("a shape");
        };
        let kind = match keyword.as_str() {
            "enum" => ShapeKind::Enum,
            "intEnum" => ShapeKind::IntEnum,
            "list" => ShapeKind::List,
            "map" => ShapeKind::Map,
            "structure" => ShapeKind::Structure,
            "union" => ShapeKind::Union,
            "service" => ShapeKind::Service,
            "resource" => ShapeKind::Resource,
            "operation" => ShapeKind::Operation,
            other => match SimpleType::parse(other) {
                Some(simple) => ShapeKind::Simple(simple),
                None => return self.error("a shape"),
            },
        };
        self.bump();
        let name = self.name("the shape's name")?;
        let mut shape = Shape {
            kind,
            name,
            traits,
            doc,
            mixins: Vec::new(),
            for_resource: None,
            members: Vec::new(),
            properties: None,
            operation: None,
        };
        self.shape_rest(&mut shape)?;
        Ok(shape)
    }

    /// What follows a shape's name: `for`, mixins and the body.
    fn shape_rest(&mut self, shape: &mut Shape) -> Parsed<()> {
        if self.is_word("for") && !matches!(shape.kind, ShapeKind::Simple(_)) {
            self.bump();
            shape.for_resource = Some(self.name("a resource")?);
        }
        if self.is_word("with") {
            self.bump();
            self.expect_symbol('[')?;
            while !self.eat_symbol(']') {
                shape.mixins.push(self.name("a mixin")?);
            }
        }
        match shape.kind {
            ShapeKind::Simple(_) => {}
            ShapeKind::Service | ShapeKind::Resource => {
                if self.is_symbol('{') {
                    shape.properties = Some(self.node(0)?);
                } else {
                    return self.error("`{`");
                }
            }
            ShapeKind::Operation => shape.operation = Some(self.operation_body(&shape.name)?),
            _ => shape.members = self.members(shape.kind)?,
        }
        Ok(())
    }

    /// `{ members }`: each with traits, a name, a target (or `$name` for an
    /// elided one) and possibly a default. Enum members have no target.
    fn members(&mut self, kind: ShapeKind) -> Parsed<Vec<Member>> {
        self.expect_symbol('{')?;
        let mut members = Vec::new();
        while !self.eat_symbol('}') {
            let doc = self.peek().doc.clone();
            let start = self.peek().span;
            let traits = self.traits()?;
            let elided = self.eat_symbol('$');
            let name = self.name("a member")?;
            let target = if elided || matches!(kind, ShapeKind::Enum | ShapeKind::IntEnum) {
                None
            } else {
                self.expect_symbol(':')?;
                Some(self.name("the member's target")?)
            };
            let default = if self.eat_symbol('=') {
                Some(self.node(1)?)
            } else {
                None
            };
            let end = self.tokens[self.pos.saturating_sub(1)].span;
            members.push(Member {
                name,
                target,
                default,
                traits,
                doc,
                span: start.join(end).unwrap_or(start),
            });
        }
        Ok(members)
    }

    fn operation_body(&mut self, name: &Name) -> Parsed<OperationBody> {
        self.expect_symbol('{')?;
        let mut body = OperationBody::default();
        while !self.eat_symbol('}') {
            let key = self.name("`input`, `output` or `errors`")?;
            match key.text.as_str() {
                "input" | "output" => {
                    let io = if self.peek().kind == TokenKind::Walrus {
                        self.bump();
                        let doc = self.peek().doc.clone();
                        let traits = self.traits()?;
                        let suffix = if key.text == "input" {
                            "Input"
                        } else {
                            "Output"
                        };
                        let mut shape = Shape {
                            kind: ShapeKind::Structure,
                            name: Name {
                                text: format!("{}{suffix}", name.text),
                                span: key.span,
                            },
                            traits,
                            doc,
                            mixins: Vec::new(),
                            for_resource: None,
                            members: Vec::new(),
                            properties: None,
                            operation: None,
                        };
                        self.shape_rest(&mut shape)?;
                        IoShape::Inline(Box::new(shape))
                    } else {
                        self.expect_symbol(':')?;
                        IoShape::Target(self.name("a structure")?)
                    };
                    if key.text == "input" {
                        body.input = Some(io);
                    } else {
                        body.output = Some(io);
                    }
                }
                "errors" => {
                    self.expect_symbol(':')?;
                    self.expect_symbol('[')?;
                    while !self.eat_symbol(']') {
                        body.errors.push(self.name("an error structure")?);
                    }
                }
                _ => {
                    self.pos -= 1;
                    return self.error("`input`, `output` or `errors`");
                }
            }
        }
        Ok(body)
    }
}

/// Whether `word` followed by `next` starts a shape statement.
fn starts_shape(word: &str, next: &TokenKind) -> bool {
    let keyword = SimpleType::parse(word).is_some()
        || matches!(
            word,
            "enum"
                | "intEnum"
                | "list"
                | "map"
                | "structure"
                | "union"
                | "service"
                | "resource"
                | "operation"
                | "apply"
        );
    keyword && matches!(next, TokenKind::Ident(_))
}
