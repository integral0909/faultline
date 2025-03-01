//! Building a schema from a document: extensions merged into their types,
//! root operation types resolved, and the type-system rules checked.

use std::collections::{HashMap, HashSet};

use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::ast::{
    Document, FieldDef, InputValue, Name, OperationKind, TypeDef, TypeDefKind, TypeRef,
};
use crate::codes;

/// Scalars every schema has without defining them.
pub const BUILTIN_SCALARS: [&str; 5] = ["Int", "Float", "String", "Boolean", "ID"];

/// A validated schema.
#[derive(Debug, Default)]
pub struct Schema {
    /// Type definitions with their extensions applied, in document order.
    pub types: Vec<TypeDef>,
    index: HashMap<String, usize>,
    /// The root type of each operation kind the schema supports.
    pub roots: Vec<(OperationKind, String)>,
}

impl Schema {
    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.index.get(name).map(|&i| &self.types[i])
    }

    /// The object types that implement the interface `name`. The
    /// specification requires objects to list every interface they
    /// implement, including inherited ones, so this is direct.
    pub fn implementations(&self, name: &str) -> Vec<&str> {
        self.types
            .iter()
            .filter(|t| {
                matches!(&t.kind, TypeDefKind::Object { interfaces, .. }
                    if interfaces.iter().any(|i| i.text == name))
            })
            .map(|t| t.name.text.as_str())
            .collect()
    }

    /// Whether `name` is a built-in scalar not redefined by the document.
    pub fn is_builtin(&self, name: &str) -> bool {
        BUILTIN_SCALARS.contains(&name) && !self.index.contains_key(name)
    }
}

/// Merges and validates `document`.
pub fn build(document: &Document) -> (Schema, Diagnostics) {
    let mut builder = Builder {
        schema: Schema::default(),
        diagnostics: Diagnostics::new(),
    };
    builder.merge(document);
    builder.roots(document);
    builder.check();
    (builder.schema, builder.diagnostics)
}

struct Builder {
    schema: Schema,
    diagnostics: Diagnostics,
}

/// Where a type may be used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Input,
    Output,
}

impl Builder {
    fn error(
        &mut self,
        code: faultline_source::DiagnosticCode,
        span: Span,
        message: String,
        label: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, label.to_owned()),
        );
    }

    // ---- merging ------------------------------------------------------

    fn merge(&mut self, document: &Document) {
        for ty in document.types.iter().filter(|t| !t.extension) {
            if let Some(&existing) = self.schema.index.get(&ty.name.text) {
                let first = self.schema.types[existing].name.span;
                self.diagnostics.push(
                    Diagnostic::error(format!("type `{}` is defined twice", ty.name.text))
                        .with_code(codes::DUPLICATE)
                        .with_primary(ty.name.span, "defined again")
                        .with_secondary(first, "first defined here"),
                );
                continue;
            }
            self.schema
                .index
                .insert(ty.name.text.clone(), self.schema.types.len());
            self.schema.types.push(ty.clone());
        }
        for extension in document.types.iter().filter(|t| t.extension) {
            self.extend(extension);
        }
    }

    fn extend(&mut self, extension: &TypeDef) {
        let Some(&index) = self.schema.index.get(&extension.name.text) else {
            self.error(
                codes::UNKNOWN_TYPE,
                extension.name.span,
                format!("cannot extend unknown type `{}`", extension.name.text),
                "not defined",
            );
            return;
        };
        let target = &mut self.schema.types[index];
        target
            .directives
            .extend(extension.directives.iter().cloned());
        let kinds_match = match (&mut target.kind, &extension.kind) {
            (TypeDefKind::Scalar, TypeDefKind::Scalar) => true,
            (
                TypeDefKind::Object { interfaces, fields },
                TypeDefKind::Object {
                    interfaces: more_interfaces,
                    fields: more_fields,
                },
            )
            | (
                TypeDefKind::Interface { interfaces, fields },
                TypeDefKind::Interface {
                    interfaces: more_interfaces,
                    fields: more_fields,
                },
            ) => {
                interfaces.extend(more_interfaces.iter().cloned());
                fields.extend(more_fields.iter().cloned());
                true
            }
            (TypeDefKind::Union { members }, TypeDefKind::Union { members: more }) => {
                members.extend(more.iter().cloned());
                true
            }
            (TypeDefKind::Enum { values }, TypeDefKind::Enum { values: more }) => {
                values.extend(more.iter().cloned());
                true
            }
            (TypeDefKind::InputObject { fields }, TypeDefKind::InputObject { fields: more }) => {
                fields.extend(more.iter().cloned());
                true
            }
            _ => false,
        };
        if !kinds_match {
            let kind = self.schema.types[index].kind.describe();
            self.error(
                codes::WRONG_KIND,
                extension.span,
                format!(
                    "`{}` is {} {kind}; an extension of it must be of the same kind",
                    extension.name.text,
                    article(kind)
                ),
                "wrong kind of extension",
            );
        }
    }

    // ---- roots --------------------------------------------------------

    fn roots(&mut self, document: &Document) {
        let mut explicit: Vec<(OperationKind, Name)> = Vec::new();
        for schema in &document.schemas {
            for (kind, name) in &schema.roots {
                if explicit.iter().any(|(k, _)| k == kind) {
                    self.error(
                        codes::DUPLICATE,
                        name.span,
                        format!("the {} root type is defined twice", kind.as_str()),
                        "defined again",
                    );
                    continue;
                }
                explicit.push((*kind, name.clone()));
            }
        }
        let schema_defined = document.schemas.iter().any(|s| !s.extension);
        let roots: Vec<(OperationKind, String, Option<Span>)> =
            if schema_defined || !explicit.is_empty() {
                explicit
                    .into_iter()
                    .map(|(kind, name)| (kind, name.text, Some(name.span)))
                    .collect()
            } else {
                [
                    OperationKind::Query,
                    OperationKind::Mutation,
                    OperationKind::Subscription,
                ]
                .into_iter()
                .filter(|kind| self.schema.index.contains_key(kind.default_type()))
                .map(|kind| (kind, kind.default_type().to_owned(), None))
                .collect()
            };
        for (kind, name, span) in roots {
            match self.schema.get(&name).map(|t| &t.kind) {
                Some(TypeDefKind::Object { .. }) => self.schema.roots.push((kind, name)),
                Some(_) => {
                    let span =
                        span.unwrap_or_else(|| self.schema.get(&name).expect("exists").name.span);
                    self.error(
                        codes::WRONG_KIND,
                        span,
                        format!(
                            "the {} root type `{name}` must be an object type",
                            kind.as_str()
                        ),
                        "not an object type",
                    );
                }
                None => {
                    if let Some(span) = span {
                        self.error(
                            codes::UNKNOWN_TYPE,
                            span,
                            format!("unknown type `{name}`"),
                            "not defined",
                        );
                    }
                }
            }
        }
        if !self
            .schema
            .roots
            .iter()
            .any(|(k, _)| *k == OperationKind::Query)
        {
            self.diagnostics.push(
                Diagnostic::error("the schema has no query root type")
                    .with_code(codes::NO_QUERY_TYPE)
                    .with_help(
                        "define `type Query { ... }`, or name one in `schema { query: ... }`",
                    ),
            );
        }
    }

    // ---- checks -------------------------------------------------------

    fn check(&mut self) {
        let types = self.schema.types.clone();
        for ty in &types {
            self.reserved(&ty.name);
            match &ty.kind {
                TypeDefKind::Scalar => {}
                TypeDefKind::Object { interfaces, fields }
                | TypeDefKind::Interface { interfaces, fields } => {
                    self.nonempty(ty, fields.len(), "fields");
                    self.fields(fields);
                    self.implementations(ty, interfaces, fields);
                }
                TypeDefKind::Union { members } => {
                    self.nonempty(ty, members.len(), "members");
                    let mut seen = HashSet::new();
                    for member in members {
                        if !seen.insert(member.text.as_str()) {
                            self.error(
                                codes::DUPLICATE,
                                member.span,
                                format!("`{}` is listed twice", member.text),
                                "listed again",
                            );
                        }
                        let kind = if self.schema.is_builtin(&member.text) {
                            Some("scalar")
                        } else {
                            self.schema.get(&member.text).map(|t| t.kind.describe())
                        };
                        match kind {
                            Some("object type") => {}
                            Some(kind) => self.error(
                                codes::WRONG_KIND,
                                member.span,
                                format!(
                                    "union members must be object types, and `{}` is {} {kind}",
                                    member.text,
                                    article(kind)
                                ),
                                "not an object type",
                            ),
                            None => self.unknown(member),
                        }
                    }
                }
                TypeDefKind::Enum { values } => {
                    self.nonempty(ty, values.len(), "values");
                    let mut seen = HashMap::new();
                    for value in values {
                        self.reserved(&value.name);
                        if let Some(first) = seen.insert(value.name.text.as_str(), value.name.span)
                        {
                            self.duplicate("enum value", &value.name, first);
                        }
                    }
                }
                TypeDefKind::InputObject { fields } => {
                    self.nonempty(ty, fields.len(), "fields");
                    self.input_values(fields);
                    if is_one_of(ty) {
                        for field in fields {
                            if field.ty.is_non_null() || field.default.is_some() {
                                self.error(
                                    codes::ONE_OF,
                                    field.name.span,
                                    format!(
                                        "`{}` is a field of the @oneOf input `{}`: it must be nullable and have no default",
                                        field.name.text, ty.name.text
                                    ),
                                    "",
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn nonempty(&mut self, ty: &TypeDef, count: usize, what: &str) {
        if count == 0 {
            self.error(
                codes::EMPTY,
                ty.name.span,
                format!("`{}` must have at least one of its {what}", ty.name.text),
                "empty",
            );
        }
    }

    fn reserved(&mut self, name: &Name) {
        if name.text.starts_with("__") {
            self.error(
                codes::RESERVED_NAME,
                name.span,
                format!(
                    "`{}` starts with `__`, which introspection reserves",
                    name.text
                ),
                "reserved",
            );
        }
    }

    fn duplicate(&mut self, what: &str, name: &Name, first: Span) {
        self.diagnostics.push(
            Diagnostic::error(format!("{what} `{}` is defined twice", name.text))
                .with_code(codes::DUPLICATE)
                .with_primary(name.span, "defined again")
                .with_secondary(first, "first defined here"),
        );
    }

    fn unknown(&mut self, name: &Name) {
        self.error(
            codes::UNKNOWN_TYPE,
            name.span,
            format!("unknown type `{}`", name.text),
            "not defined",
        );
    }

    fn fields(&mut self, fields: &[FieldDef]) {
        let mut seen = HashMap::new();
        for field in fields {
            self.reserved(&field.name);
            if let Some(first) = seen.insert(field.name.text.as_str(), field.name.span) {
                self.duplicate("field", &field.name, first);
            }
            self.type_ref(&field.ty, Position::Output);
            self.input_values(&field.arguments);
        }
    }

    fn input_values(&mut self, values: &[InputValue]) {
        let mut seen = HashMap::new();
        for value in values {
            self.reserved(&value.name);
            if let Some(first) = seen.insert(value.name.text.as_str(), value.name.span) {
                self.duplicate("argument", &value.name, first);
            }
            self.type_ref(&value.ty, Position::Input);
        }
    }

    fn type_ref(&mut self, ty: &TypeRef, position: Position) {
        let base = ty.base();
        if self.schema.is_builtin(&base.text) {
            return;
        }
        let Some(def) = self.schema.get(&base.text) else {
            self.unknown(base);
            return;
        };
        let allowed = matches!(
            (&def.kind, position),
            (TypeDefKind::Scalar | TypeDefKind::Enum { .. }, _)
                | (TypeDefKind::InputObject { .. }, Position::Input)
                | (
                    TypeDefKind::Object { .. }
                        | TypeDefKind::Interface { .. }
                        | TypeDefKind::Union { .. },
                    Position::Output
                )
        );
        if !allowed {
            let kind = def.kind.describe();
            let (place, wants) = match position {
                Position::Input => ("an argument or input field", "input types"),
                Position::Output => ("a field", "output types"),
            };
            self.error(
                codes::WRONG_KIND,
                base.span,
                format!(
                    "`{}` is {} {kind}, but {place} needs one of the {wants}",
                    base.text,
                    article(kind)
                ),
                "wrong kind of type",
            );
        }
    }

    /// Every interface a type implements must be an interface, and its
    /// fields (with their arguments) must be provided.
    fn implementations(&mut self, ty: &TypeDef, interfaces: &[Name], fields: &[FieldDef]) {
        for interface in interfaces {
            let Some(def) = self.schema.get(&interface.text) else {
                self.unknown(interface);
                continue;
            };
            let TypeDefKind::Interface {
                fields: required, ..
            } = &def.kind
            else {
                let kind = def.kind.describe();
                self.error(
                    codes::WRONG_KIND,
                    interface.span,
                    format!(
                        "`{}` is {} {kind}, not an interface",
                        interface.text,
                        article(kind)
                    ),
                    "not an interface",
                );
                continue;
            };
            let required = required.clone();
            for field in &required {
                let Some(provided) = fields.iter().find(|f| f.name.text == field.name.text) else {
                    self.diagnostics.push(
                        Diagnostic::error(format!(
                            "`{}` does not provide the field `{}` of interface `{}`",
                            ty.name.text, field.name.text, interface.text
                        ))
                        .with_code(codes::INTERFACE)
                        .with_primary(ty.name.span, "missing a field")
                        .with_secondary(field.name.span, "declared here"),
                    );
                    continue;
                };
                for argument in &field.arguments {
                    if !provided
                        .arguments
                        .iter()
                        .any(|a| a.name.text == argument.name.text)
                    {
                        self.diagnostics.push(
                            Diagnostic::error(format!(
                                "`{}.{}` lacks the argument `{}` of interface `{}`",
                                ty.name.text, field.name.text, argument.name.text, interface.text
                            ))
                            .with_code(codes::INTERFACE)
                            .with_primary(provided.name.span, "missing an argument"),
                        );
                    }
                }
            }
        }
    }
}

fn article(kind: &str) -> &'static str {
    if kind.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// Whether an input object is `@oneOf`: exactly one of its fields is set.
pub(crate) fn is_one_of(ty: &TypeDef) -> bool {
    ty.directives.iter().any(|d| d.name.text == "oneOf")
}
