//! The HTTP/JSON view of a gRPC service.
//!
//! Services published through HTTP/JSON transcoding (gRPC-Gateway, Envoy's
//! transcoder, Google Cloud Endpoints) annotate each RPC with an
//! `option (google.api.http)` rule saying which method and path template
//! expose it, which part of the request is the body and which part of the
//! response is returned. Lowering a `.proto` file as its HTTP view produces
//! the API a JSON client sees, the same shape an OpenAPI description of
//! that API has, so the two can be compared: with the OpenAPI description
//! as the old version and the view as the new one, any finding is a promise
//! of the description that the service does not keep.
//!
//! For each rule and each of its `additional_bindings`:
//!
//! * the path template becomes the operation's path, with
//!   `{field=pattern}` variables written `{field}`; each variable is a
//!   required path parameter holding the field's JSON value;
//! * with `body: "*"`, the request message minus the path fields is the
//!   body; with `body: "field"`, that field is the body and the remaining
//!   fields are query parameters; without a body, every remaining field is
//!   a query parameter, nested messages flattened into dotted names;
//! * the response message (or its `response_body` field) is the `200`
//!   response.
//!
//! Operation ids are `Service_Method`, as gRPC-Gateway's OpenAPI generator
//! writes them. RPCs without a rule, and streaming RPCs, are not exposed.

use std::collections::HashSet;

use faultline_ir::{
    Api, ApiFormat, Content, HttpMethod, Meta, ObjectType, Operation, OperationKey, ParamLocation,
    Parameter, PathTemplate, RequestBody, Response, StatusKey, TypeId, TypeKind, TypeNode,
};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::ast::{self, OptionValue, option};
use crate::codes;
use crate::json::{JsonTypes, field_json_name, is_required};
use crate::linker::{Linked, MessageDecl};
use crate::load::ProtoFile;

/// The option holding an RPC's HTTP rule.
pub const HTTP_OPTION: &str = "(google.api.http)";

/// The media type of transcoded bodies.
pub const JSON_MEDIA_TYPE: &str = "application/json";

/// Nested messages are flattened into query parameters to this depth.
const MAX_QUERY_DEPTH: usize = 8;

/// One HTTP binding of an RPC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRule {
    /// Upper-case method name (`GET`, or a custom kind).
    pub method: String,
    /// The path template as written: `/v1/{name=shelves/*}/books`.
    pub path: String,
    /// `*`, a field name, or `None` for no body.
    pub body: Option<String>,
    pub response_body: Option<String>,
    pub additional_bindings: Vec<HttpRule>,
}

/// Parses the text-format value of a `google.api.http` option.
pub fn parse_rule(text: &str) -> Result<HttpRule, String> {
    let mut parser = TextParser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let fields = parser.message(true)?;
    rule_from(&fields)
}

fn rule_from(fields: &[(String, TextValue)]) -> Result<HttpRule, String> {
    let mut rule = HttpRule {
        method: String::new(),
        path: String::new(),
        body: None,
        response_body: None,
        additional_bindings: Vec::new(),
    };
    for (name, value) in fields {
        match (name.as_str(), value) {
            ("get" | "put" | "post" | "delete" | "patch", TextValue::Str(path)) => {
                rule.method = name.to_ascii_uppercase();
                rule.path.clone_from(path);
            }
            ("custom", TextValue::Message(custom)) => {
                for (key, value) in custom {
                    match (key.as_str(), value) {
                        ("kind", TextValue::Str(kind)) => rule.method = kind.to_ascii_uppercase(),
                        ("path", TextValue::Str(path)) => rule.path.clone_from(path),
                        _ => {}
                    }
                }
            }
            ("body", TextValue::Str(body)) => {
                rule.body = Some(body.clone()).filter(|b| !b.is_empty());
            }
            ("response_body", TextValue::Str(body)) => {
                rule.response_body = Some(body.clone()).filter(|b| !b.is_empty());
            }
            ("additional_bindings", TextValue::Message(inner)) => {
                rule.additional_bindings.push(rule_from(inner)?);
            }
            ("selector", _) => {}
            (other, _) => return Err(format!("unexpected `{other}` in an HTTP rule")),
        }
    }
    if rule.method.is_empty() || rule.path.is_empty() {
        return Err("an HTTP rule needs a method and a path, such as `get: \"/v1/pets\"`".into());
    }
    Ok(rule)
}

#[derive(Clone, Debug, PartialEq)]
enum TextValue {
    Str(String),
    Other(String),
    Message(Vec<(String, TextValue)>),
}

/// Protobuf text format, as much of it as option values use.
struct TextParser {
    chars: Vec<char>,
    pos: usize,
}

impl TextParser {
    fn skip_space(&mut self) {
        while let Some(&c) = self.chars.get(self.pos) {
            if c.is_whitespace() {
                self.pos += 1;
            } else if c == '#' {
                while self.chars.get(self.pos).is_some_and(|&c| c != '\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_space();
        self.chars.get(self.pos).copied()
    }

    /// Fields up to the closing brace (or the end, for the outermost
    /// message, whose braces are part of the option value).
    fn message(&mut self, outermost: bool) -> Result<Vec<(String, TextValue)>, String> {
        let mut close = None;
        if let Some(open @ ('{' | '<')) = self.peek() {
            self.pos += 1;
            close = Some(if open == '{' { '}' } else { '>' });
        } else if !outermost {
            return Err("expected `{`".into());
        }
        let mut fields = Vec::new();
        loop {
            match self.peek() {
                None if close.is_none() => return Ok(fields),
                None => return Err("unterminated message".into()),
                Some(c) if Some(c) == close => {
                    self.pos += 1;
                    return Ok(fields);
                }
                Some(',' | ';') => self.pos += 1,
                Some(_) => {
                    let name = self.identifier()?;
                    let mut colon = false;
                    if self.peek() == Some(':') {
                        self.pos += 1;
                        colon = true;
                    }
                    let value = match self.peek() {
                        Some('{' | '<') => TextValue::Message(self.message(false)?),
                        Some('"' | '\'') if colon => TextValue::Str(self.strings()?),
                        Some(_) if colon => TextValue::Other(self.scalar()),
                        _ => return Err(format!("expected a value for `{name}`")),
                    };
                    fields.push((name, value));
                }
            }
        }
    }

    fn identifier(&mut self) -> Result<String, String> {
        let start = self.pos;
        while self
            .chars
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.'))
        {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(format!(
                "expected a field name at `{}`",
                self.chars[self.pos..].iter().take(10).collect::<String>()
            ));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn scalar(&mut self) -> String {
        let start = self.pos;
        while self
            .chars
            .get(self.pos)
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, ',' | ';' | '}' | '>'))
        {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    /// One or more adjacent string literals, concatenated.
    fn strings(&mut self) -> Result<String, String> {
        let mut out = String::new();
        while let Some(quote @ ('"' | '\'')) = self.peek() {
            self.pos += 1;
            loop {
                let Some(&c) = self.chars.get(self.pos) else {
                    return Err("unterminated string".into());
                };
                self.pos += 1;
                match c {
                    c if c == quote => break,
                    '\\' => {
                        let Some(&escaped) = self.chars.get(self.pos) else {
                            return Err("unterminated string".into());
                        };
                        self.pos += 1;
                        out.push(match escaped {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                    c => out.push(c),
                }
            }
        }
        Ok(out)
    }
}

/// A path template's HTTP path (`{field=pattern}` written `{field}`) and the
/// field paths its variables bind.
pub fn path_template(template: &str) -> Result<(String, Vec<String>), String> {
    if !template.starts_with('/') {
        return Err(format!(
            "the path template `{template}` must start with `/`"
        ));
    }
    let mut path = String::new();
    let mut fields = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        path.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            return Err(format!("unclosed `{{` in `{template}`"));
        };
        let variable = &rest[open + 1..open + close];
        let field = variable.split_once('=').map_or(variable, |(f, _)| f).trim();
        if field.is_empty()
            || !field
                .split('.')
                .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err(format!("invalid variable `{{{variable}}}` in `{template}`"));
        }
        path.push('{');
        path.push_str(field);
        path.push('}');
        fields.push(field.to_owned());
        rest = &rest[open + close + 1..];
    }
    path.push_str(rest);
    Ok((path, fields))
}

/// Lowers the entry file's services as their HTTP/JSON view.
pub(crate) fn lower(files: &[ProtoFile], linked: &Linked, name: &str) -> (Api, Diagnostics) {
    let mut view = View {
        api: Api::new(name, ApiFormat::Protobuf),
        json: JsonTypes::new(linked),
        linked,
        diagnostics: Diagnostics::new(),
        seen: HashSet::new(),
    };
    let services: Vec<&String> = linked.order[0]
        .iter()
        .filter(|name| linked.services.contains_key(*name))
        .collect();
    if services.is_empty() {
        view.diagnostics.push(
            Diagnostic::warning(format!(
                "{} declares no services, so it exposes no HTTP operations",
                files[0].name
            ))
            .with_code(codes::HTTP_RULE),
        );
    }
    for service in services {
        view.service(service);
    }
    view.json.finish(&mut view.api);
    (view.api, view.diagnostics)
}

struct View<'a> {
    api: Api,
    json: JsonTypes<'a>,
    linked: &'a Linked,
    diagnostics: Diagnostics,
    /// Operation identities already bound, to report duplicate bindings.
    seen: HashSet<String>,
}

impl<'a> View<'a> {
    fn warn(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::warning(message)
                .with_code(codes::HTTP_RULE)
                .with_primary(span, ""),
        );
    }

    fn service(&mut self, full_name: &str) {
        let service = &self.linked.services[full_name];
        let short = full_name.rsplit('.').next().unwrap_or(full_name);
        for method in &service.ast.methods {
            let Some(option) = option(&method.options, HTTP_OPTION) else {
                continue;
            };
            if method.input_stream || method.output_stream {
                self.warn(
                    format!(
                        "{short}.{} streams, and streaming RPCs are not compared over HTTP",
                        method.name.text
                    ),
                    method.name.span,
                );
                continue;
            }
            let OptionValue::Aggregate(text) = &option.value else {
                self.warn("the HTTP rule must be a message value".into(), option.span);
                continue;
            };
            let rule = match parse_rule(text) {
                Ok(rule) => rule,
                Err(e) => {
                    self.warn(format!("invalid HTTP rule: {e}"), option.span);
                    continue;
                }
            };
            let input = self.message(&method.input);
            let output = self.message(&method.output);
            let operation_id = format!("{short}_{}", method.name.text);
            let mut bindings = vec![rule.clone()];
            bindings.extend(rule.additional_bindings.iter().cloned());
            for binding in &bindings {
                if let Some(mut op) = self.operation(binding, input, output, option.span) {
                    op.operation_id = Some(operation_id.clone());
                    op.meta.description.clone_from(&method.doc);
                    op.meta.deprecated = ast::option(&method.options, "deprecated")
                        .and_then(|o| o.value.as_bool())
                        .unwrap_or(false);
                    op.origin = Some(option.span);
                    self.api.operations.push(op);
                }
            }
        }
    }

    fn message(&self, name: &ast::TypeName) -> &'a MessageDecl {
        let resolved = self
            .linked
            .resolve(name)
            .expect("the linker resolved every RPC type");
        &self.linked.messages[&resolved.full_name]
    }

    fn operation(
        &mut self,
        rule: &HttpRule,
        input: &'a MessageDecl,
        output: &'a MessageDecl,
        span: Span,
    ) -> Option<Operation> {
        let Some(method) = HttpMethod::parse(&rule.method) else {
            self.warn(
                format!("custom method {} cannot be compared", rule.method),
                span,
            );
            return None;
        };
        let (path, bound) = match path_template(&rule.path) {
            Ok(template) => template,
            Err(e) => {
                self.warn(e, span);
                return None;
            }
        };
        let key = OperationKey::Http {
            method,
            path: PathTemplate::parse(&path),
        };
        if !self.seen.insert(key.identity()) {
            self.warn(
                format!("{} is bound to more than one RPC", key.identity()),
                span,
            );
            return None;
        }
        let mut op = Operation::new(key);

        for field_path in &bound {
            let Some(field) = self.field_at(input, field_path) else {
                self.warn(
                    format!("the path variable `{field_path}` names no field of the request"),
                    span,
                );
                return None;
            };
            let ty = self.json.field_type(&mut self.api, field);
            op.parameters
                .push(parameter(ParamLocation::Path, field_path, true, ty, field));
        }
        // Top-level fields the path binds whole are not also in the body or
        // the query.
        let in_path: HashSet<&str> = bound
            .iter()
            .filter(|p| !p.contains('.'))
            .map(String::as_str)
            .collect();

        let body = match rule.body.as_deref() {
            None => {
                self.query(
                    input,
                    &in_path,
                    None,
                    "",
                    0,
                    &mut op.parameters,
                    &mut HashSet::new(),
                );
                None
            }
            Some("*") => Some(self.body_without(input, &in_path)),
            Some(name) => {
                let Some(field) = input.ast.fields.iter().find(|f| f.name.text == name) else {
                    self.warn(
                        format!("the body `{name}` names no field of the request"),
                        span,
                    );
                    return None;
                };
                let ty = self.json.field_type(&mut self.api, field);
                self.query(
                    input,
                    &in_path,
                    Some(name),
                    "",
                    0,
                    &mut op.parameters,
                    &mut HashSet::new(),
                );
                Some(ty)
            }
        };
        op.request_body = body.map(|ty| RequestBody {
            // An empty body decodes as an empty message.
            required: false,
            content: vec![Content {
                media_type: JSON_MEDIA_TYPE.to_owned(),
                ty,
                origin: Some(input.ast.name.span),
            }],
            meta: Meta::default(),
            origin: Some(span),
        });

        op.responses.push(self.response(rule, output, span)?);
        Some(op)
    }

    /// The `200` response: the response message, or its `response_body`
    /// field.
    fn response(
        &mut self,
        rule: &HttpRule,
        output: &'a MessageDecl,
        span: Span,
    ) -> Option<Response> {
        let response = match rule.response_body.as_deref() {
            None => self.json.named(&mut self.api, &output.full_name),
            Some(name) => {
                let Some(field) = output.ast.fields.iter().find(|f| f.name.text == name) else {
                    self.warn(
                        format!("the response body `{name}` names no field of the response"),
                        span,
                    );
                    return None;
                };
                self.json.field_type(&mut self.api, field)
            }
        };
        Some(Response {
            status: StatusKey::Code(200),
            headers: Vec::new(),
            content: vec![Content {
                media_type: JSON_MEDIA_TYPE.to_owned(),
                ty: response,
                origin: Some(output.ast.name.span),
            }],
            meta: Meta::default(),
            origin: Some(output.ast.name.span),
        })
    }

    /// The field a dotted path names, through nested messages.
    fn field_at(&self, message: &'a MessageDecl, path: &str) -> Option<&'a ast::Field> {
        let mut current = message;
        let mut segments = path.split('.').peekable();
        while let Some(segment) = segments.next() {
            let field = current.ast.fields.iter().find(|f| f.name.text == segment)?;
            if segments.peek().is_none() {
                return Some(field);
            }
            current = self.json.message_of(field)?;
        }
        None
    }

    /// The request message as a body, without the fields the path binds.
    fn body_without(&mut self, input: &'a MessageDecl, in_path: &HashSet<&str>) -> TypeId {
        if !input
            .ast
            .fields
            .iter()
            .any(|f| in_path.contains(f.name.text.as_str()))
        {
            return self.json.named(&mut self.api, &input.full_name);
        }
        let additional = self.api.types.alloc_kind(TypeKind::Any);
        let mut object = ObjectType::new(additional);
        for field in &input.ast.fields {
            if in_path.contains(field.name.text.as_str()) {
                continue;
            }
            let ty = self.json.field_type(&mut self.api, field);
            let mut out =
                faultline_ir::Field::new(field_json_name(field), ty, is_required(&field.options));
            out.origin = Some(field.span);
            object.fields.push(out);
        }
        self.api
            .types
            .alloc(TypeNode::new(TypeKind::Object(object)).with_origin(input.ast.name.span))
    }

    /// Query parameters for the fields of `message` not bound elsewhere:
    /// scalars, enums and repeated scalars by JSON name, singular messages
    /// flattened into dotted names.
    #[allow(clippy::too_many_arguments)]
    fn query(
        &mut self,
        message: &'a MessageDecl,
        in_path: &HashSet<&str>,
        body: Option<&str>,
        prefix: &str,
        depth: usize,
        out: &mut Vec<Parameter>,
        visiting: &mut HashSet<String>,
    ) {
        if depth > MAX_QUERY_DEPTH || !visiting.insert(message.full_name.clone()) {
            return;
        }
        for field in &message.ast.fields {
            if depth == 0
                && (in_path.contains(field.name.text.as_str()) || body == Some(&field.name.text))
            {
                continue;
            }
            let name = format!("{prefix}{}", field_json_name(field));
            let repeated = field.label == Some(ast::Label::Repeated);
            if matches!(field.ty, ast::FieldType::Map(..)) {
                continue;
            }
            if let Some(nested) = self.json.message_of(field) {
                if !repeated {
                    let prefix = format!("{name}.");
                    self.query(nested, in_path, body, &prefix, depth + 1, out, visiting);
                }
                continue;
            }
            let ty = self.json.field_type(&mut self.api, field);
            out.push(parameter(
                ParamLocation::Query,
                &name,
                is_required(&field.options),
                ty,
                field,
            ));
        }
        visiting.remove(&message.full_name);
    }
}

fn parameter(
    location: ParamLocation,
    name: &str,
    required: bool,
    ty: TypeId,
    field: &ast::Field,
) -> Parameter {
    Parameter {
        location,
        name: name.to_owned(),
        required,
        ty,
        serialization: None,
        meta: Meta {
            description: field.doc.clone(),
            ..Meta::default()
        },
        origin: Some(field.span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let rule = parse_rule(
            r#"{
                get: "/v1/{name=shelves/*}"
                additional_bindings { post: "/v1/shelves:" "lookup" body: "*" }
                additional_bindings < custom { kind: "HEAD" path: "/v1/ping" } >
                response_body: 'shelf', # the shelf itself
            }"#,
        )
        .unwrap();
        assert_eq!(rule.method, "GET");
        assert_eq!(rule.path, "/v1/{name=shelves/*}");
        assert_eq!(rule.response_body.as_deref(), Some("shelf"));
        assert_eq!(rule.additional_bindings.len(), 2);
        assert_eq!(rule.additional_bindings[0].method, "POST");
        assert_eq!(rule.additional_bindings[0].path, "/v1/shelves:lookup");
        assert_eq!(rule.additional_bindings[0].body.as_deref(), Some("*"));
        assert_eq!(rule.additional_bindings[1].method, "HEAD");
        for bad in [
            "{}",
            "{ get: \"/a\" frobnicate: 1 }",
            "{ get: \"/a\"",
            "{ get: \"/a }",
            "{ get \"/a\" }",
        ] {
            assert!(parse_rule(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn templates() {
        assert_eq!(
            path_template("/v1/{name=shelves/*/books/*}:archive").unwrap(),
            ("/v1/{name}:archive".to_owned(), vec!["name".to_owned()])
        );
        assert_eq!(
            path_template("/v1/{book.shelf}/books/{book.id}").unwrap(),
            (
                "/v1/{book.shelf}/books/{book.id}".to_owned(),
                vec!["book.shelf".to_owned(), "book.id".to_owned()]
            )
        );
        assert!(path_template("v1/x").is_err());
        assert!(path_template("/v1/{name").is_err());
        assert!(path_template("/v1/{}").is_err());
        assert!(path_template("/v1/{a..b}").is_err());
    }
}
