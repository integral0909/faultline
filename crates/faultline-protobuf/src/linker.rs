//! Name resolution and semantic checks across a set of loaded files.
//!
//! Every message, enum, service and package gets a fully-qualified name.
//! Type references are resolved the way `protoc` resolves them: the first
//! component of a name is looked up from the innermost enclosing scope
//! outwards, and once found, the rest of the name must exist inside it. A
//! reference may only name types from its own file, the files it imports,
//! and files those re-export with `import public`.

use std::collections::{HashMap, HashSet};

use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::ast::{
    self, Enum, Field, FieldType, Label, Message, Scalar, Service, Syntax, TypeName, option,
};
use crate::codes;
use crate::load::ProtoFile;
use crate::parser::MAX_FIELD_NUMBER;

/// Field numbers Protobuf reserves for its own implementation.
const IMPLEMENTATION_RESERVED: (i64, i64) = (19_000, 19_999);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Package,
    Message,
    Enum,
    EnumValue,
    Service,
}

impl SymbolKind {
    /// The description with its indefinite article.
    fn with_article(self) -> &'static str {
        match self {
            SymbolKind::Package => "a package",
            SymbolKind::Message => "a message",
            SymbolKind::Enum => "an enum",
            SymbolKind::EnumValue => "an enum value",
            SymbolKind::Service => "a service",
        }
    }

    /// Whether names can be looked up inside it.
    fn is_scope(self) -> bool {
        matches!(self, SymbolKind::Package | SymbolKind::Message)
    }
}

#[derive(Clone, Debug)]
struct Symbol {
    kind: SymbolKind,
    file: usize,
    span: Span,
}

/// A message with the context lowering needs.
#[derive(Clone, Debug)]
pub struct MessageDecl {
    pub full_name: String,
    pub file: usize,
    pub syntax: Syntax,
    pub ast: Message,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub full_name: String,
    pub file: usize,
    pub syntax: Syntax,
    /// Closed enums (proto2, or `features.enum_type = CLOSED`) reject
    /// unknown numbers; open ones keep them.
    pub closed: bool,
    pub ast: Enum,
}

#[derive(Clone, Debug)]
pub struct ServiceDecl {
    pub full_name: String,
    pub file: usize,
    pub ast: Service,
}

/// What a type reference resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub full_name: String,
    pub kind: SymbolKind,
}

/// The result of linking.
#[derive(Debug, Default)]
pub struct Linked {
    pub messages: HashMap<String, MessageDecl>,
    pub enums: HashMap<String, EnumDecl>,
    pub services: HashMap<String, ServiceDecl>,
    /// Declaration order of top-level and nested names per file, for
    /// deterministic output.
    pub order: Vec<Vec<String>>,
    /// Type references by the span of the reference.
    pub resolved: HashMap<Span, Resolved>,
    pub diagnostics: Diagnostics,
}

impl Linked {
    pub fn resolve(&self, name: &TypeName) -> Option<&Resolved> {
        self.resolved.get(&name.span)
    }
}

/// Links `files` (the entry file first).
pub fn link(files: &[ProtoFile]) -> Linked {
    let mut linker = Linker {
        files,
        symbols: HashMap::new(),
        linked: Linked {
            order: vec![Vec::new(); files.len()],
            ..Linked::default()
        },
        visible: Vec::new(),
        exported: (0..files.len()).map(|i| public_closure(files, i)).collect(),
        used_imports: vec![HashSet::new(); files.len()],
    };
    for (index, file) in files.iter().enumerate() {
        linker.declare_file(index, file);
    }
    linker.visible = (0..files.len())
        .map(|i| {
            let mut visible = HashSet::from([i]);
            for imported in files[i].imports.iter().flatten() {
                visible.extend(linker.exported[*imported].iter().copied());
            }
            visible
        })
        .collect();
    for (index, file) in files.iter().enumerate() {
        linker.check_file(index, file);
    }
    linker.report_unused_imports();
    linker.linked
}

struct Linker<'a> {
    files: &'a [ProtoFile],
    symbols: HashMap<String, Symbol>,
    linked: Linked,
    /// Per file: the files whose declarations it may reference.
    visible: Vec<HashSet<usize>>,
    /// Per file: itself and the files it re-exports through `import public`.
    exported: Vec<HashSet<usize>>,
    /// Per file: the imports (by index into `ast.imports`) that were needed.
    used_imports: Vec<HashSet<usize>>,
}

fn package_of(file: &ProtoFile) -> String {
    file.ast
        .package
        .as_ref()
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}.{name}")
    }
}

/// The file and everything it re-exports through chains of public imports:
/// what importing it makes visible.
fn public_closure(files: &[ProtoFile], index: usize) -> HashSet<usize> {
    let mut visible = HashSet::new();
    let mut stack = vec![index];
    while let Some(f) = stack.pop() {
        if !visible.insert(f) {
            continue;
        }
        for (import, target) in files[f].ast.imports.iter().zip(&files[f].imports) {
            if import.kind == ast::ImportKind::Public
                && let Some(target) = target
            {
                stack.push(*target);
            }
        }
    }
    visible
}

impl Linker<'_> {
    // ---- declarations -------------------------------------------------

    fn define(&mut self, name: String, kind: SymbolKind, file: usize, span: Span) -> bool {
        match self.symbols.get(&name) {
            // A package may be declared by any number of files.
            Some(existing)
                if existing.kind == SymbolKind::Package && kind == SymbolKind::Package =>
            {
                false
            }
            Some(existing) => {
                let existing = existing.clone();
                let in_other_file = existing.file != file;
                let mut d = Diagnostic::error(format!(
                    "`{name}` is already defined as {}",
                    existing.kind.with_article()
                ))
                .with_code(codes::DUPLICATE_SYMBOL)
                .with_primary(span, "redefined here");
                if in_other_file {
                    d = d.with_note(format!(
                        "the first definition is in {}",
                        self.files[existing.file].name
                    ));
                } else {
                    d = d.with_secondary(existing.span, "first defined here");
                }
                self.linked.diagnostics.push(d);
                false
            }
            None => {
                self.symbols.insert(name, Symbol { kind, file, span });
                true
            }
        }
    }

    fn declare_file(&mut self, index: usize, file: &ProtoFile) {
        let package = package_of(file);
        if let Some(p) = &file.ast.package {
            // Each prefix of the package is a package too.
            let mut prefix = String::new();
            for part in p.text.split('.') {
                prefix = qualify(&prefix, part);
                self.define(prefix.clone(), SymbolKind::Package, index, p.span);
            }
        }
        let syntax = file.ast.syntax.clone();
        for message in &file.ast.messages {
            self.declare_message(index, &package, message, &syntax);
        }
        for e in &file.ast.enums {
            self.declare_enum(index, &package, e, &syntax, file);
        }
        for service in &file.ast.services {
            let full_name = qualify(&package, &service.name.text);
            if self.define(
                full_name.clone(),
                SymbolKind::Service,
                index,
                service.name.span,
            ) {
                self.linked.order[index].push(full_name.clone());
                self.linked.services.insert(
                    full_name.clone(),
                    ServiceDecl {
                        full_name,
                        file: index,
                        ast: service.clone(),
                    },
                );
            }
        }
    }

    fn declare_message(&mut self, file: usize, scope: &str, message: &Message, syntax: &Syntax) {
        let full_name = qualify(scope, &message.name.text);
        if !self.define(
            full_name.clone(),
            SymbolKind::Message,
            file,
            message.name.span,
        ) {
            return;
        }
        self.linked.order[file].push(full_name.clone());
        self.linked.messages.insert(
            full_name.clone(),
            MessageDecl {
                full_name: full_name.clone(),
                file,
                syntax: syntax.clone(),
                ast: message.clone(),
            },
        );
        for nested in &message.messages {
            self.declare_message(file, &full_name, nested, syntax);
        }
        let files = self.files;
        let source = &files[file];
        for e in &message.enums {
            self.declare_enum(file, &full_name, e, syntax, source);
        }
    }

    fn declare_enum(
        &mut self,
        file: usize,
        scope: &str,
        e: &Enum,
        syntax: &Syntax,
        source: &ProtoFile,
    ) {
        let full_name = qualify(scope, &e.name.text);
        if !self.define(full_name.clone(), SymbolKind::Enum, file, e.name.span) {
            return;
        }
        // Enum values are siblings of their enum (C++ scoping), so two enums
        // in one scope cannot share a value name.
        for value in &e.values {
            self.define(
                qualify(scope, &value.name.text),
                SymbolKind::EnumValue,
                file,
                value.name.span,
            );
        }
        let feature = |options: &[ast::OptionDecl]| {
            option(options, "features.enum_type")
                .and_then(|o| o.value.as_ident())
                .map(|v| v == "CLOSED")
        };
        let closed = feature(&e.options)
            .or_else(|| feature(&source.ast.options))
            .unwrap_or(*syntax == Syntax::Proto2);
        self.linked.order[file].push(full_name.clone());
        self.linked.enums.insert(
            full_name.clone(),
            EnumDecl {
                full_name,
                file,
                syntax: syntax.clone(),
                closed,
                ast: e.clone(),
            },
        );
    }

    // ---- resolution ---------------------------------------------------

    /// Resolves `name` referenced from `scope` in `file`.
    fn resolve(&mut self, file: usize, scope: &str, name: &TypeName) -> Option<Resolved> {
        let full = if name.absolute {
            let full = name.parts.join(".");
            self.symbols.contains_key(&full).then_some(full)
        } else {
            self.resolve_relative(scope, name)
        };
        let Some(full) = full else {
            self.linked.diagnostics.push(
                Diagnostic::error(format!("cannot find type `{name}`"))
                    .with_code(codes::UNRESOLVED_TYPE)
                    .with_primary(name.span, "not found"),
            );
            return None;
        };
        let symbol = self.symbols[&full].clone();
        if !self.visible[file].contains(&symbol.file) {
            self.linked.diagnostics.push(
                Diagnostic::error(format!(
                    "`{full}` is defined in {}, which is not imported",
                    self.files[symbol.file].name
                ))
                .with_code(codes::UNRESOLVED_TYPE)
                .with_primary(name.span, "not visible here")
                .with_help(format!(
                    "add `import \"{}\";`",
                    self.files[symbol.file].name
                )),
            );
            return None;
        }
        self.mark_used(file, symbol.file);
        let resolved = Resolved {
            full_name: full,
            kind: symbol.kind,
        };
        self.linked.resolved.insert(name.span, resolved.clone());
        Some(resolved)
    }

    /// Scope search: the first component from the innermost scope outwards;
    /// the first scope that has it decides, provided it can contain the
    /// rest of the name.
    fn resolve_relative(&self, scope: &str, name: &TypeName) -> Option<String> {
        let first = &name.parts[0];
        let rest = &name.parts[1..];
        let mut scope = scope.to_owned();
        loop {
            let candidate = qualify(&scope, first);
            if let Some(symbol) = self.symbols.get(&candidate)
                && (rest.is_empty() || symbol.kind.is_scope())
            {
                let full = rest.iter().fold(candidate, |acc, part| qualify(&acc, part));
                // Found the first component: the rest must be inside it.
                return self.symbols.contains_key(&full).then_some(full);
            }
            if scope.is_empty() {
                return None;
            }
            scope = scope
                .rfind('.')
                .map_or_else(String::new, |i| scope[..i].to_owned());
        }
    }

    fn mark_used(&mut self, file: usize, target: usize) {
        if file == target {
            return;
        }
        let imports = &self.files[file].imports;
        for (i, import) in imports.iter().enumerate() {
            if let Some(imported) = import
                && self.exported[*imported].contains(&target)
            {
                self.used_imports[file].insert(i);
            }
        }
    }

    fn report_unused_imports(&mut self) {
        for (index, file) in self.files.iter().enumerate() {
            // Only the entry file's hygiene is the user's concern here; a
            // shared file imported from elsewhere is checked where it is
            // the entry.
            if index != 0 {
                continue;
            }
            let options = custom_options(&file.ast);
            for (i, import) in file.ast.imports.iter().enumerate() {
                let Some(imported) = file.imports[i] else {
                    continue;
                };
                if import.kind != ast::ImportKind::Default || self.used_imports[index].contains(&i)
                {
                    continue;
                }
                // Imports that declare the extensions custom options name
                // (`(google.api.http)`) are used by those options.
                let provides_option = self.exported[imported].iter().any(|&f| {
                    extensions(&self.files[f].ast).iter().any(|e| {
                        options
                            .iter()
                            .any(|o| e == o || e.ends_with(&format!(".{o}")))
                    })
                });
                if provides_option {
                    continue;
                }
                self.linked.diagnostics.push(
                    Diagnostic::warning(format!("\"{}\" is imported but not used", import.path))
                        .with_code(codes::UNUSED_IMPORT)
                        .with_primary(import.span, "unused import"),
                );
            }
        }
    }

    // ---- checks -------------------------------------------------------

    fn check_file(&mut self, index: usize, file: &ProtoFile) {
        let package = package_of(file);
        for message in &file.ast.messages {
            self.check_message(index, &package, message, &file.ast.syntax);
        }
        for e in &file.ast.enums {
            self.check_enum(e, &file.ast.syntax);
        }
        for service in &file.ast.services {
            for method in &service.methods {
                for ty in [&method.input, &method.output] {
                    if let Some(resolved) = self.resolve(index, &package, ty)
                        && resolved.kind != SymbolKind::Message
                    {
                        self.wrong_kind(ty, &resolved, "an RPC input or output must be a message");
                    }
                }
            }
        }
        for extend in &file.ast.extends {
            self.check_extend(index, &package, extend);
        }
    }

    fn wrong_kind(&mut self, ty: &TypeName, resolved: &Resolved, message: &str) {
        self.linked.diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(codes::WRONG_KIND)
                .with_primary(
                    ty.span,
                    format!(
                        "`{}` is {}",
                        resolved.full_name,
                        resolved.kind.with_article()
                    ),
                ),
        );
    }

    fn check_extend(&mut self, file: usize, scope: &str, extend: &ast::Extend) {
        if let Some(resolved) = self.resolve(file, scope, &extend.extendee)
            && resolved.kind != SymbolKind::Message
        {
            self.wrong_kind(&extend.extendee, &resolved, "only messages can be extended");
        }
        for field in &extend.fields {
            self.check_field_type(file, scope, field);
        }
    }

    fn check_message(&mut self, file: usize, scope: &str, message: &Message, syntax: &Syntax) {
        let full_name = qualify(scope, &message.name.text);
        let mut numbers: HashMap<i64, Span> = HashMap::new();
        let mut names: HashMap<&str, Span> = HashMap::new();
        for field in &message.fields {
            self.check_field_type(file, &full_name, field);
            self.check_field_rules(field, syntax);
            self.check_number(field);
            if let Some(first) = numbers.insert(field.number, field.number_span) {
                self.linked.diagnostics.push(
                    Diagnostic::error(format!("field number {} is used twice", field.number))
                        .with_code(codes::DUPLICATE_NUMBER)
                        .with_primary(field.number_span, "used again")
                        .with_secondary(first, "first used here"),
                );
            }
            if let Some(first) = names.insert(field.name.text.as_str(), field.name.span) {
                self.linked.diagnostics.push(
                    Diagnostic::error(format!("field `{}` is declared twice", field.name.text))
                        .with_code(codes::DUPLICATE_SYMBOL)
                        .with_primary(field.name.span, "declared again")
                        .with_secondary(first, "first declared here"),
                );
            }
            for reserved in &message.reserved {
                let by_number = reserved
                    .ranges
                    .iter()
                    .any(|&(lo, hi)| (lo..=hi).contains(&field.number));
                let by_name = reserved.names.contains(&field.name.text);
                if by_number || by_name {
                    let what = if by_number {
                        format!("field number {}", field.number)
                    } else {
                        format!("field name `{}`", field.name.text)
                    };
                    self.linked.diagnostics.push(
                        Diagnostic::error(format!("{what} is reserved"))
                            .with_code(codes::RESERVED_CONFLICT)
                            .with_primary(
                                if by_number {
                                    field.number_span
                                } else {
                                    field.name.span
                                },
                                "uses a reserved value",
                            )
                            .with_secondary(reserved.span, "reserved here"),
                    );
                }
            }
        }
        if *syntax == Syntax::Proto3 && !message.extension_ranges.is_empty() {
            self.syntax_error(
                message.name.span,
                "extension ranges are not allowed in proto3 messages",
            );
        }
        for nested in &message.messages {
            self.check_message(file, &full_name, nested, syntax);
        }
        for e in &message.enums {
            self.check_enum(e, syntax);
        }
        for extend in &message.extends {
            self.check_extend(file, &full_name, extend);
        }
    }

    fn check_field_type(&mut self, file: usize, scope: &str, field: &Field) {
        match &field.ty {
            FieldType::Scalar(_) => {}
            FieldType::Named(name) => self.check_named_field_type(file, scope, name),
            FieldType::Map(key, value) => {
                match key.as_ref() {
                    FieldType::Scalar(s) if s.is_map_key() => {}
                    _ => self.linked.diagnostics.push(
                        Diagnostic::error("map keys must be integers, `bool` or `string`")
                            .with_code(codes::INVALID_MAP_KEY)
                            .with_primary(field.span, "in this map"),
                    ),
                }
                if field.label.is_some() {
                    self.syntax_error(field.span, "map fields cannot have a label");
                }
                match value.as_ref() {
                    FieldType::Named(name) => self.check_named_field_type(file, scope, name),
                    FieldType::Map(..) => {
                        self.syntax_error(field.span, "map values cannot be maps");
                    }
                    FieldType::Scalar(_) => {}
                }
            }
        }
    }

    fn check_named_field_type(&mut self, file: usize, scope: &str, name: &TypeName) {
        if let Some(resolved) = self.resolve(file, scope, name)
            && !matches!(resolved.kind, SymbolKind::Message | SymbolKind::Enum)
        {
            self.wrong_kind(name, &resolved, "a field type must be a message or an enum");
        }
    }

    /// Constructs the syntax level forbids.
    fn check_field_rules(&mut self, field: &Field, syntax: &Syntax) {
        if *syntax == Syntax::Proto2 {
            if field.label.is_none()
                && field.oneof.is_none()
                && !matches!(field.ty, FieldType::Map(..))
            {
                self.syntax_error(
                    field.span,
                    "proto2 fields need a label: `optional`, `required` or `repeated`",
                );
            }
            return;
        }
        if field.label == Some(Label::Required) {
            self.syntax_error(field.span, "`required` is only allowed in proto2");
        }
        if field.group {
            self.syntax_error(field.span, "groups are only allowed in proto2");
        }
        if *syntax == Syntax::Proto3 && option(&field.options, "default").is_some() {
            self.syntax_error(field.span, "default values are only allowed in proto2");
        }
    }

    fn check_number(&mut self, field: &Field) {
        let n = field.number;
        let problem = if !(1..=MAX_FIELD_NUMBER).contains(&n) {
            Some(format!(
                "field numbers must be between 1 and {MAX_FIELD_NUMBER}"
            ))
        } else if (IMPLEMENTATION_RESERVED.0..=IMPLEMENTATION_RESERVED.1).contains(&n) {
            Some(format!(
                "field numbers {} to {} are reserved for the Protobuf implementation",
                IMPLEMENTATION_RESERVED.0, IMPLEMENTATION_RESERVED.1
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            self.linked.diagnostics.push(
                Diagnostic::error(problem)
                    .with_code(codes::INVALID_NUMBER)
                    .with_primary(field.number_span, format!("{n} is not allowed")),
            );
        }
    }

    fn check_enum(&mut self, e: &Enum, syntax: &Syntax) {
        if e.values.is_empty() {
            self.syntax_error(e.name.span, "an enum needs at least one value");
            return;
        }
        if *syntax != Syntax::Proto2 && e.values[0].number != 0 {
            self.linked.diagnostics.push(
                Diagnostic::error("the first value of an open enum must be zero")
                    .with_code(codes::INVALID_FOR_SYNTAX)
                    .with_primary(e.values[0].name.span, "must be `= 0`")
                    .with_note("zero is the default for unset fields"),
            );
        }
        let alias = option(&e.options, "allow_alias")
            .and_then(|o| o.value.as_bool())
            .unwrap_or(false);
        let mut seen: HashMap<i64, Span> = HashMap::new();
        for value in &e.values {
            if i32::try_from(value.number).is_err() {
                self.linked.diagnostics.push(
                    Diagnostic::error("enum values must fit in 32 bits")
                        .with_code(codes::INVALID_NUMBER)
                        .with_primary(value.span, "out of range"),
                );
            }
            match seen.get(&value.number) {
                Some(&first) if !alias => self.linked.diagnostics.push(
                    Diagnostic::error(format!(
                        "enum number {} is used twice without `allow_alias`",
                        value.number
                    ))
                    .with_code(codes::DUPLICATE_NUMBER)
                    .with_primary(value.span, "used again")
                    .with_secondary(first, "first used here"),
                ),
                Some(_) => {}
                None => {
                    seen.insert(value.number, value.span);
                }
            }
            for reserved in &e.reserved {
                if reserved
                    .ranges
                    .iter()
                    .any(|&(lo, hi)| (lo..=hi).contains(&value.number))
                    || reserved.names.contains(&value.name.text)
                {
                    self.linked.diagnostics.push(
                        Diagnostic::error(format!("enum value `{}` is reserved", value.name.text))
                            .with_code(codes::RESERVED_CONFLICT)
                            .with_primary(value.span, "uses a reserved value")
                            .with_secondary(reserved.span, "reserved here"),
                    );
                }
            }
        }
    }

    fn syntax_error(&mut self, span: Span, message: &str) {
        self.linked.diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(codes::INVALID_FOR_SYNTAX)
                .with_primary(span, "here"),
        );
    }
}

/// The extensions custom options in `file` name: `(google.api.http)` and
/// `(google.api.http).get` both name `google.api.http`.
fn custom_options(file: &ast::File) -> HashSet<String> {
    fn add(options: &[ast::OptionDecl], out: &mut HashSet<String>) {
        for option in options {
            if let Some(rest) = option.name.strip_prefix('(')
                && let Some((name, _)) = rest.split_once(')')
            {
                out.insert(name.trim_start_matches('.').to_owned());
            }
        }
    }
    fn message(m: &ast::Message, out: &mut HashSet<String>) {
        add(&m.options, out);
        for field in &m.fields {
            add(&field.options, out);
        }
        for oneof in &m.oneofs {
            add(&oneof.options, out);
        }
        for e in &m.enums {
            enumeration(e, out);
        }
        for nested in &m.messages {
            message(nested, out);
        }
    }
    fn enumeration(e: &ast::Enum, out: &mut HashSet<String>) {
        add(&e.options, out);
        for value in &e.values {
            add(&value.options, out);
        }
    }
    let mut out = HashSet::new();
    add(&file.options, &mut out);
    for m in &file.messages {
        message(m, &mut out);
    }
    for e in &file.enums {
        enumeration(e, &mut out);
    }
    for service in &file.services {
        add(&service.options, &mut out);
        for method in &service.methods {
            add(&method.options, &mut out);
        }
    }
    out
}

/// The full names of the extensions `file` declares at its top level.
fn extensions(file: &ast::File) -> Vec<String> {
    let package = file.package.as_ref().map_or("", |p| p.text.as_str());
    file.extends
        .iter()
        .flat_map(|extend| &extend.fields)
        .map(|field| qualify(package, &field.name.text))
        .collect()
}

/// The lowerCamelCase JSON name `protoc` derives from a field name.
pub fn json_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A scalar's wire encoding, as recorded in the IR.
pub fn encoding(scalar: Scalar) -> Option<&'static str> {
    Some(match scalar {
        Scalar::Int32 | Scalar::Int64 | Scalar::Uint32 | Scalar::Uint64 => "varint",
        Scalar::Sint32 | Scalar::Sint64 => "zigzag",
        Scalar::Fixed32 | Scalar::Sfixed32 => "fixed32",
        Scalar::Fixed64 | Scalar::Sfixed64 => "fixed64",
        Scalar::Float => "float",
        Scalar::Double => "double",
        Scalar::Bool | Scalar::String | Scalar::Bytes => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_names_follow_protoc() {
        assert_eq!(json_name("user_id"), "userId");
        assert_eq!(json_name("name"), "name");
        assert_eq!(json_name("a__b"), "aB");
        assert_eq!(json_name("_private"), "Private");
        assert_eq!(json_name("x_1_y"), "x1Y");
    }
}
