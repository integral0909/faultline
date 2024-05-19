//! Recursive-descent parser for `.proto` files.
//!
//! Protobuf keywords are contextual (`message` is a valid field name), so
//! statements are recognized by lookahead rather than by reserved words.
//! A syntax error abandons the current statement: the parser skips to the
//! next `;` or to the end of the enclosing block and carries on, so one typo
//! produces one diagnostic and the rest of the file is still read.

use faultline_source::{Diagnostic, Diagnostics, SourceId, Span};

use crate::ast::{
    Enum, EnumValue, Extend, Field, FieldType, File, Import, ImportKind, Label, Message, Method,
    Name, Oneof, OptionDecl, OptionValue, Reserved, Scalar, Service, Syntax, TypeName,
};
use crate::codes;
use crate::lexer::{Token, TokenKind, tokenize};

/// Nesting deeper than this (messages in messages, aggregate braces) is
/// reported instead of parsed, so hostile input cannot exhaust the stack.
const MAX_DEPTH: usize = 128;

/// Largest field number Protobuf allows.
pub const MAX_FIELD_NUMBER: i64 = (1 << 29) - 1;

/// Parses `text`, the contents of the file registered as `source`.
pub fn parse(source: SourceId, text: &str) -> (File, Diagnostics) {
    let mut parser = Parser {
        text,
        tokens: tokenize(source, text),
        pos: 0,
        depth: 0,
        diagnostics: Diagnostics::new(),
    };
    let file = parser.file();
    (file, parser.diagnostics)
}

/// Why a statement was abandoned; the diagnostic is already recorded.
struct Abandoned;

type PResult<T> = Result<T, Abandoned>;

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    diagnostics: Diagnostics,
}

impl Parser<'_> {
    // ---- token access -------------------------------------------------

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_at(&self, ahead: usize) -> &TokenKind {
        &self.tokens[(self.pos + ahead).min(self.tokens.len() - 1)].kind
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        token
    }

    fn at_symbol(&self, c: char) -> bool {
        self.peek().kind == TokenKind::Symbol(c)
    }

    fn at_ident(&self, word: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(s) if s == word)
    }

    fn eat_symbol(&mut self, c: char) -> bool {
        let at = self.at_symbol(c);
        if at {
            self.pos += 1;
        }
        at
    }

    fn eat_ident(&mut self, word: &str) -> bool {
        let at = self.at_ident(word);
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

    // ---- errors -------------------------------------------------------

    fn error_at(&mut self, span: Span, message: impl Into<String>, label: impl Into<String>) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::SYNTAX)
                .with_primary(span, label),
        );
    }

    /// Reports what was found where `expected` was needed.
    fn unexpected<T>(&mut self, expected: &str) -> PResult<T> {
        let token = self.peek().clone();
        let found = match &token.kind {
            TokenKind::Ident(s) => format!("`{s}`"),
            TokenKind::Int(n) => format!("number {n}"),
            TokenKind::Float(f) => format!("number {f}"),
            TokenKind::Str(_) => "a string".to_owned(),
            TokenKind::Symbol(c) => format!("`{c}`"),
            TokenKind::Error(message) => {
                let message = message.clone();
                self.error_at(token.span, message, "here");
                return Err(Abandoned);
            }
            TokenKind::Eof => "the end of the file".to_owned(),
        };
        self.error_at(
            token.span,
            format!("expected {expected}, found {found}"),
            format!("expected {expected}"),
        );
        Err(Abandoned)
    }

    fn expect_symbol(&mut self, c: char) -> PResult<Span> {
        if self.at_symbol(c) {
            Ok(self.bump().span)
        } else {
            self.unexpected(&format!("`{c}`"))
        }
    }

    fn expect(&mut self, c: char) -> PResult<()> {
        self.expect_symbol(c).map(|_| ())
    }

    fn expect_ident(&mut self, what: &str) -> PResult<Name> {
        match &self.peek().kind {
            TokenKind::Ident(s) => {
                let text = s.clone();
                let span = self.bump().span;
                Ok(Name { text, span })
            }
            _ => self.unexpected(what),
        }
    }

    /// Skips the rest of a statement: to just after the next `;` at this
    /// level, or to (not past) the `}` closing the enclosing block. A
    /// nested block that starts in the statement is skipped whole.
    fn recover(&mut self) {
        let mut depth = 0usize;
        loop {
            match &self.peek().kind {
                TokenKind::Eof => return,
                TokenKind::Symbol(';') if depth == 0 => {
                    self.pos += 1;
                    return;
                }
                TokenKind::Symbol('{') => depth += 1,
                TokenKind::Symbol('}') => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    if depth == 0 {
                        self.pos += 1;
                        return;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    // ---- file ---------------------------------------------------------

    fn file(&mut self) -> File {
        let mut file = File {
            syntax: Syntax::Proto2,
            package: None,
            imports: Vec::new(),
            options: Vec::new(),
            messages: Vec::new(),
            enums: Vec::new(),
            services: Vec::new(),
            extends: Vec::new(),
        };
        // `syntax` or `edition` must come first.
        if (self.at_ident("syntax") || self.at_ident("edition"))
            && self.peek_at(1) == &TokenKind::Symbol('=')
        {
            match self.syntax() {
                Ok(syntax) => file.syntax = syntax,
                Err(Abandoned) => self.recover(),
            }
        }
        while !matches!(self.peek().kind, TokenKind::Eof) {
            if self.at_symbol('}') {
                let span = self.bump().span;
                self.error_at(span, "unmatched `}`", "no block to close");
                continue;
            }
            if let Err(Abandoned) = self.top_level(&mut file) {
                self.recover();
            }
        }
        file
    }

    fn syntax(&mut self) -> PResult<Syntax> {
        let keyword = self.bump();
        self.expect('=')?;
        let value = self.string_value()?;
        self.expect(';')?;
        let span = self.span_from(keyword.span);
        let text = String::from_utf8_lossy(&value).into_owned();
        let is_edition = keyword.kind == TokenKind::Ident("edition".into());
        Ok(match (is_edition, text.as_str()) {
            (false, "proto2") => Syntax::Proto2,
            (false, "proto3") => Syntax::Proto3,
            (true, _) => Syntax::Edition(text),
            (false, other) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("unknown syntax \"{other}\""))
                        .with_code(codes::INVALID_FOR_SYNTAX)
                        .with_primary(span, "expected \"proto2\" or \"proto3\""),
                );
                Syntax::Proto3
            }
        })
    }

    fn top_level(&mut self, file: &mut File) -> PResult<()> {
        let doc = self.peek().doc.clone();
        if self.eat_symbol(';') {
            return Ok(());
        }
        let TokenKind::Ident(word) = self.peek().kind.clone() else {
            return self.unexpected("a declaration");
        };
        match word.as_str() {
            "package" => {
                let start = self.bump().span;
                let name = self.dotted_name()?;
                self.expect(';')?;
                if let Some(previous) = &file.package {
                    let previous = previous.span;
                    self.diagnostics.push(
                        Diagnostic::error("a file has one package")
                            .with_code(codes::SYNTAX)
                            .with_primary(self.span_from(start), "second package")
                            .with_secondary(previous, "first package"),
                    );
                } else {
                    file.package = Some(name);
                }
            }
            "import" => {
                let start = self.bump().span;
                let kind = if self.eat_ident("public") {
                    ImportKind::Public
                } else if self.eat_ident("weak") {
                    ImportKind::Weak
                } else {
                    ImportKind::Default
                };
                let path = self.string_value()?;
                self.expect(';')?;
                file.imports.push(Import {
                    path: String::from_utf8_lossy(&path).into_owned(),
                    kind,
                    span: self.span_from(start),
                });
            }
            "option" => {
                let option = self.option_statement()?;
                file.options.push(option);
            }
            "message" => {
                let message = self.message(doc)?;
                file.messages.push(message);
            }
            "enum" => {
                let e = self.enumeration(doc)?;
                file.enums.push(e);
            }
            "service" => {
                let service = self.service(doc)?;
                file.services.push(service);
            }
            "extend" => {
                let extend = self.extend()?;
                file.extends.push(extend);
            }
            "syntax" | "edition" => {
                let span = self.peek().span;
                self.error_at(
                    span,
                    format!("`{word}` must be the first statement of the file"),
                    "too late",
                );
                return Err(Abandoned);
            }
            _ => return self.unexpected("`message`, `enum`, `service` or another declaration"),
        }
        Ok(())
    }

    // ---- names and values ---------------------------------------------

    /// `a.b.c`
    fn dotted_name(&mut self) -> PResult<Name> {
        let first = self.expect_ident("a name")?;
        let mut text = first.text;
        let mut span = first.span;
        while self.at_symbol('.') {
            self.bump();
            let part = self.expect_ident("a name after `.`")?;
            text.push('.');
            text.push_str(&part.text);
            span = span.join(part.span).unwrap_or(span);
        }
        Ok(Name { text, span })
    }

    /// `.a.b.C` or `B.C`
    fn type_name(&mut self) -> PResult<TypeName> {
        let start = self.peek().span;
        let absolute = self.eat_symbol('.');
        let first = self.expect_ident("a type name")?;
        let mut parts = vec![first.text];
        while self.at_symbol('.') {
            self.bump();
            parts.push(self.expect_ident("a name after `.`")?.text);
        }
        Ok(TypeName {
            absolute,
            parts,
            span: self.span_from(start),
        })
    }

    /// One or more adjacent string literals, concatenated.
    fn string_value(&mut self) -> PResult<Vec<u8>> {
        let TokenKind::Str(first) = self.peek().kind.clone() else {
            return self.unexpected("a string");
        };
        self.bump();
        let mut value = first;
        while let TokenKind::Str(more) = &self.peek().kind {
            value.extend_from_slice(more);
            self.bump();
        }
        Ok(value)
    }

    /// A possibly signed integer.
    fn integer(&mut self, what: &str) -> PResult<(i128, Span)> {
        let start = self.peek().span;
        let negative = self.eat_symbol('-');
        if !negative {
            self.eat_symbol('+');
        }
        match self.peek().kind {
            TokenKind::Int(n) => {
                self.bump();
                let value = i128::from(n);
                Ok((if negative { -value } else { value }, self.span_from(start)))
            }
            _ => self.unexpected(what),
        }
    }

    /// `name` in `option name = ...`: `a.b`, `(a.b).c`, `(.a.b)`.
    fn option_name(&mut self) -> PResult<String> {
        let mut name = String::new();
        loop {
            if self.eat_symbol('(') {
                name.push('(');
                if self.eat_symbol('.') {
                    name.push('.');
                }
                name.push_str(&self.dotted_name()?.text);
                self.expect(')')?;
                name.push(')');
            } else {
                name.push_str(&self.expect_ident("an option name")?.text);
            }
            if !self.eat_symbol('.') {
                return Ok(name);
            }
            name.push('.');
        }
    }

    fn option_value(&mut self) -> PResult<OptionValue> {
        let start = self.peek().span;
        let negative = self.eat_symbol('-');
        if !negative {
            self.eat_symbol('+');
        }
        let token = self.peek().clone();
        let value = match token.kind {
            TokenKind::Int(n) => {
                let value = i128::from(n);
                OptionValue::Int(if negative { -value } else { value })
            }
            TokenKind::Float(f) => OptionValue::Float(if negative { -f } else { f }),
            TokenKind::Ident(ref s) if negative && (s == "inf" || s == "nan") => {
                OptionValue::Float(if s == "inf" {
                    f64::NEG_INFINITY
                } else {
                    f64::NAN
                })
            }
            TokenKind::Ident(ref s) if !negative => OptionValue::Ident(s.clone()),
            TokenKind::Str(_) if !negative => {
                return Ok(OptionValue::Str(self.string_value()?));
            }
            TokenKind::Symbol('{') if !negative => {
                self.skip_braces()?;
                let span = self.span_from(start);
                let text = &self.text[span.start() as usize..span.end() as usize];
                return Ok(OptionValue::Aggregate(text.to_owned()));
            }
            _ => return self.unexpected("an option value"),
        };
        self.bump();
        Ok(value)
    }

    /// Skips a balanced `{ ... }`.
    fn skip_braces(&mut self) -> PResult<()> {
        let open = self.expect_symbol('{')?;
        let mut depth = 1usize;
        loop {
            match self.peek().kind {
                TokenKind::Eof => {
                    self.error_at(open, "unclosed `{`", "opened here");
                    return Err(Abandoned);
                }
                TokenKind::Symbol('{') => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        let span = self.peek().span;
                        self.error_at(span, "value is nested too deeply", "here");
                        return Err(Abandoned);
                    }
                }
                TokenKind::Symbol('}') => {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        return Ok(());
                    }
                }
                _ => {}
            }
            self.bump();
        }
    }

    /// `option name = value;`
    fn option_statement(&mut self) -> PResult<OptionDecl> {
        let start = self.bump().span;
        let name = self.option_name()?;
        self.expect('=')?;
        let value = self.option_value()?;
        self.expect(';')?;
        Ok(OptionDecl {
            name,
            value,
            span: self.span_from(start),
        })
    }

    /// `[a = 1, b = "x"]`, if present.
    fn bracket_options(&mut self) -> PResult<Vec<OptionDecl>> {
        let mut options = Vec::new();
        if !self.eat_symbol('[') {
            return Ok(options);
        }
        loop {
            let start = self.peek().span;
            let name = self.option_name()?;
            self.expect('=')?;
            let value = self.option_value()?;
            options.push(OptionDecl {
                name,
                value,
                span: self.span_from(start),
            });
            if self.eat_symbol(']') {
                return Ok(options);
            }
            self.expect(',')?;
        }
    }

    // ---- messages -----------------------------------------------------

    fn enter(&mut self, span: Span) -> PResult<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.error_at(span, "declarations are nested too deeply", "here");
            return Err(Abandoned);
        }
        Ok(())
    }

    fn message(&mut self, doc: Option<String>) -> PResult<Message> {
        let start = self.bump().span;
        let name = self.expect_ident("a message name")?;
        let mut message = Message::new(name, start, doc);
        self.message_body(&mut message)?;
        message.span = self.span_from(start);
        Ok(message)
    }

    fn message_body(&mut self, message: &mut Message) -> PResult<()> {
        let open = self.expect_symbol('{')?;
        self.enter(open)?;
        let result = self.message_statements(message, open);
        self.depth -= 1;
        result
    }

    fn message_statements(&mut self, message: &mut Message, open: Span) -> PResult<()> {
        loop {
            if self.eat_symbol('}') {
                return Ok(());
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                self.error_at(open, "unclosed `{`", "opened here");
                return Err(Abandoned);
            }
            if let Err(Abandoned) = self.message_statement(message) {
                self.recover();
            }
        }
    }

    /// Whether the statement at the cursor is an option. As in protoc,
    /// `option` followed by a name always starts one, so no field can have
    /// a type named `option`.
    fn at_option(&self) -> bool {
        self.at_ident("option")
            && matches!(
                self.peek_at(1),
                TokenKind::Ident(_) | TokenKind::Symbol('(')
            )
    }

    /// Whether the statement at the cursor is `<keyword> Name {`, as
    /// opposed to a field whose type happens to be named like a keyword.
    fn at_block(&self, keyword: &str) -> bool {
        self.at_ident(keyword)
            && matches!(self.peek_at(1), TokenKind::Ident(_))
            && self.peek_at(2) == &TokenKind::Symbol('{')
    }

    fn message_statement(&mut self, message: &mut Message) -> PResult<()> {
        let doc = self.peek().doc.clone();
        if self.eat_symbol(';') {
            return Ok(());
        }
        if self.at_block("message") {
            let nested = self.message(doc)?;
            message.messages.push(nested);
        } else if self.at_block("enum") {
            let e = self.enumeration(doc)?;
            message.enums.push(e);
        } else if self.at_block("oneof") {
            self.oneof(message, doc)?;
        } else if self.at_ident("extend") && !matches!(self.peek_at(1), TokenKind::Symbol('=')) {
            let extend = self.extend()?;
            message.extends.push(extend);
        } else if self.at_option() {
            let option = self.option_statement()?;
            message.options.push(option);
        } else if self.at_ident("reserved")
            && !(matches!(self.peek_at(1), TokenKind::Ident(_))
                && self.peek_at(2) == &TokenKind::Symbol('='))
        {
            let reserved = self.reserved()?;
            message.reserved.push(reserved);
        } else if self.at_ident("extensions") && matches!(self.peek_at(1), TokenKind::Int(_)) {
            self.bump();
            let reserved = self.ranges()?;
            let _ = self.bracket_options()?;
            self.expect(';')?;
            message.extension_ranges.extend(reserved);
        } else {
            let field = self.field(doc, message, None)?;
            message.fields.push(field);
        }
        Ok(())
    }

    /// A field, map field or group. Groups add a nested message.
    fn field(
        &mut self,
        doc: Option<String>,
        message: &mut Message,
        oneof: Option<usize>,
    ) -> PResult<Field> {
        let start = self.peek().span;
        let label = if oneof.is_some() {
            None
        } else if self.eat_ident("optional") {
            Some(Label::Optional)
        } else if self.eat_ident("required") {
            Some(Label::Required)
        } else if self.eat_ident("repeated") {
            Some(Label::Repeated)
        } else {
            None
        };

        if self.at_ident("group") && matches!(self.peek_at(1), TokenKind::Ident(_)) {
            return self.group(doc, message, label, oneof, start);
        }

        let ty = if self.at_ident("map") && self.peek_at(1) == &TokenKind::Symbol('<') {
            self.bump();
            self.bump();
            let key = self.field_type()?;
            self.expect(',')?;
            let value = self.field_type()?;
            self.expect('>')?;
            FieldType::Map(Box::new(key), Box::new(value))
        } else {
            self.field_type()?
        };
        let name = self.expect_ident("a field name")?;
        self.expect('=')?;
        let (number, number_span) = self.integer("a field number")?;
        let options = self.bracket_options()?;
        self.expect(';')?;
        Ok(Field {
            label,
            ty,
            name,
            number: clamp(number),
            number_span,
            options,
            doc,
            span: self.span_from(start),
            oneof,
            group: false,
        })
    }

    /// `group Name = 1 { ... }`: a nested message `Name` and a field
    /// `name` of that type.
    fn group(
        &mut self,
        doc: Option<String>,
        message: &mut Message,
        label: Option<Label>,
        oneof: Option<usize>,
        start: Span,
    ) -> PResult<Field> {
        self.bump();
        let type_name = self.expect_ident("a group name")?;
        if !type_name.text.starts_with(|c: char| c.is_ascii_uppercase()) {
            self.error_at(
                type_name.span,
                "a group name must start with a capital letter",
                "here",
            );
        }
        self.expect('=')?;
        let (number, number_span) = self.integer("a field number")?;
        let options = self.bracket_options()?;
        let mut body = Message::new(type_name.clone(), type_name.span, doc.clone());
        self.message_body(&mut body)?;
        body.span = self.span_from(start);
        message.messages.push(body);
        Ok(Field {
            label,
            ty: FieldType::Named(TypeName {
                absolute: false,
                parts: vec![type_name.text.clone()],
                span: type_name.span,
            }),
            name: Name {
                text: type_name.text.to_ascii_lowercase(),
                span: type_name.span,
            },
            number: clamp(number),
            number_span,
            options,
            doc,
            span: self.span_from(start),
            oneof,
            group: true,
        })
    }

    fn field_type(&mut self) -> PResult<FieldType> {
        if let TokenKind::Ident(word) = &self.peek().kind
            && let Some(scalar) = Scalar::parse(word)
            && self.peek_at(1) != &TokenKind::Symbol('.')
        {
            self.bump();
            return Ok(FieldType::Scalar(scalar));
        }
        Ok(FieldType::Named(self.type_name()?))
    }

    fn oneof(&mut self, message: &mut Message, doc: Option<String>) -> PResult<()> {
        let start = self.bump().span;
        let name = self.expect_ident("a oneof name")?;
        let index = message.oneofs.len();
        message.oneofs.push(Oneof {
            name,
            doc,
            span: start,
            options: Vec::new(),
        });
        let open = self.expect_symbol('{')?;
        loop {
            if self.eat_symbol('}') {
                break;
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                self.error_at(open, "unclosed `{`", "opened here");
                return Err(Abandoned);
            }
            let doc = self.peek().doc.clone();
            let result = if self.eat_symbol(';') {
                Ok(())
            } else if self.at_option() {
                self.option_statement()
                    .map(|option| message.oneofs[index].options.push(option))
            } else {
                if let Some(label @ ("optional" | "required" | "repeated")) =
                    match &self.peek().kind {
                        TokenKind::Ident(s) => Some(s.as_str()),
                        _ => None,
                    }
                {
                    let span = self.peek().span;
                    self.error_at(
                        span,
                        format!("fields in a oneof cannot be `{label}`"),
                        "remove this label",
                    );
                    self.bump();
                }
                self.field(doc, message, Some(index))
                    .map(|field| message.fields.push(field))
            };
            if let Err(Abandoned) = result {
                self.recover();
            }
        }
        message.oneofs[index].span = self.span_from(start);
        Ok(())
    }

    /// `reserved 1, 5 to 9, 100 to max;` or `reserved "a", "b";` (editions
    /// write names as identifiers).
    fn reserved(&mut self) -> PResult<Reserved> {
        let start = self.bump().span;
        let mut reserved = Reserved {
            ranges: Vec::new(),
            names: Vec::new(),
            span: start,
        };
        match self.peek().kind {
            TokenKind::Str(_) | TokenKind::Ident(_) => loop {
                let name = match self.peek().kind.clone() {
                    TokenKind::Str(_) => {
                        String::from_utf8_lossy(&self.string_value()?).into_owned()
                    }
                    TokenKind::Ident(s) => {
                        self.bump();
                        s
                    }
                    _ => return self.unexpected("a reserved name"),
                };
                reserved.names.push(name);
                if !self.eat_symbol(',') {
                    break;
                }
            },
            _ => reserved.ranges = self.ranges()?,
        }
        self.expect(';')?;
        reserved.span = self.span_from(start);
        Ok(reserved)
    }

    /// `1, 5 to 9, 100 to max`
    fn ranges(&mut self) -> PResult<Vec<(i64, i64)>> {
        let mut ranges = Vec::new();
        loop {
            let (low, _) = self.integer("a number")?;
            let high = if self.eat_ident("to") {
                if self.eat_ident("max") {
                    i64::MAX
                } else {
                    clamp(self.integer("a number or `max`")?.0)
                }
            } else {
                clamp(low)
            };
            ranges.push((clamp(low), high));
            if !self.eat_symbol(',') {
                return Ok(ranges);
            }
        }
    }

    // ---- enums --------------------------------------------------------

    fn enumeration(&mut self, doc: Option<String>) -> PResult<Enum> {
        let start = self.bump().span;
        let name = self.expect_ident("an enum name")?;
        let mut e = Enum {
            name,
            doc,
            span: start,
            values: Vec::new(),
            reserved: Vec::new(),
            options: Vec::new(),
        };
        let open = self.expect_symbol('{')?;
        loop {
            if self.eat_symbol('}') {
                break;
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                self.error_at(open, "unclosed `{`", "opened here");
                return Err(Abandoned);
            }
            if let Err(Abandoned) = self.enum_statement(&mut e) {
                self.recover();
            }
        }
        e.span = self.span_from(start);
        Ok(e)
    }

    fn enum_statement(&mut self, e: &mut Enum) -> PResult<()> {
        let doc = self.peek().doc.clone();
        if self.eat_symbol(';') {
            return Ok(());
        }
        if self.at_ident("option") && self.peek_at(1) != &TokenKind::Symbol('=') {
            let option = self.option_statement()?;
            e.options.push(option);
            return Ok(());
        }
        if self.at_ident("reserved") && self.peek_at(1) != &TokenKind::Symbol('=') {
            let reserved = self.reserved()?;
            e.reserved.push(reserved);
            return Ok(());
        }
        let start = self.peek().span;
        let name = self.expect_ident("an enum value name")?;
        self.expect('=')?;
        let (number, _) = self.integer("an enum value number")?;
        let options = self.bracket_options()?;
        self.expect(';')?;
        e.values.push(EnumValue {
            name,
            number: clamp(number),
            doc,
            span: self.span_from(start),
            options,
        });
        Ok(())
    }

    // ---- services -----------------------------------------------------

    fn service(&mut self, doc: Option<String>) -> PResult<Service> {
        let start = self.bump().span;
        let name = self.expect_ident("a service name")?;
        let mut service = Service {
            name,
            doc,
            span: start,
            methods: Vec::new(),
            options: Vec::new(),
        };
        let open = self.expect_symbol('{')?;
        loop {
            if self.eat_symbol('}') {
                break;
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                self.error_at(open, "unclosed `{`", "opened here");
                return Err(Abandoned);
            }
            let doc = self.peek().doc.clone();
            let result = if self.eat_symbol(';') {
                Ok(())
            } else if self.at_option() {
                self.option_statement()
                    .map(|option| service.options.push(option))
            } else if self.at_ident("rpc") {
                self.method(doc).map(|method| service.methods.push(method))
            } else {
                self.unexpected("`rpc` or `option`")
            };
            if let Err(Abandoned) = result {
                self.recover();
            }
        }
        service.span = self.span_from(start);
        Ok(service)
    }

    fn method(&mut self, doc: Option<String>) -> PResult<Method> {
        let start = self.bump().span;
        let name = self.expect_ident("a method name")?;
        self.expect('(')?;
        let input_stream =
            self.at_ident("stream") && !matches!(self.peek_at(1), TokenKind::Symbol(')'));
        if input_stream {
            self.bump();
        }
        let input = self.type_name()?;
        self.expect(')')?;
        if !self.eat_ident("returns") {
            return self.unexpected("`returns`");
        }
        self.expect('(')?;
        let output_stream =
            self.at_ident("stream") && !matches!(self.peek_at(1), TokenKind::Symbol(')'));
        if output_stream {
            self.bump();
        }
        let output = self.type_name()?;
        self.expect(')')?;
        let mut options = Vec::new();
        if self.at_symbol('{') {
            let open = self.bump().span;
            loop {
                if self.eat_symbol('}') {
                    break;
                }
                if matches!(self.peek().kind, TokenKind::Eof) {
                    self.error_at(open, "unclosed `{`", "opened here");
                    return Err(Abandoned);
                }
                if self.eat_symbol(';') {
                    continue;
                }
                if !self.at_ident("option") {
                    return self.unexpected("`option`");
                }
                options.push(self.option_statement()?);
            }
            // A trailing `;` after the block is allowed.
            self.eat_symbol(';');
        } else {
            self.expect(';')?;
        }
        Ok(Method {
            name,
            doc,
            span: self.span_from(start),
            input,
            input_stream,
            output,
            output_stream,
            options,
        })
    }

    fn extend(&mut self) -> PResult<Extend> {
        let start = self.bump().span;
        let extendee = self.type_name()?;
        let mut holder = Message::new(
            Name {
                text: extendee.to_string(),
                span: extendee.span,
            },
            start,
            None,
        );
        let open = self.expect_symbol('{')?;
        loop {
            if self.eat_symbol('}') {
                break;
            }
            if matches!(self.peek().kind, TokenKind::Eof) {
                self.error_at(open, "unclosed `{`", "opened here");
                return Err(Abandoned);
            }
            let doc = self.peek().doc.clone();
            if self.eat_symbol(';') {
                continue;
            }
            match self.field(doc, &mut holder, None) {
                Ok(field) => holder.fields.push(field),
                Err(Abandoned) => self.recover(),
            }
        }
        Ok(Extend {
            extendee,
            fields: holder.fields,
            span: self.span_from(start),
        })
    }
}

/// Numbers outside `i64` are out of range for every use; they are clamped so
/// range checks report them.
fn clamp(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}
