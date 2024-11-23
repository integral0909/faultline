//! Resolving names across files and checking what the compiler checks.
//!
//! A name is a definition of the same file (`Pet`) or of an included one,
//! qualified by that file's prefix (`shared.Tag`). The linker reports
//! duplicate definitions, field ids that are missing, repeated or out of
//! range, names that resolve to nothing or to the wrong kind of
//! definition, and typedefs or services that refer to themselves.

use std::collections::{HashMap, HashSet};

use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::ast::{Definition, Field, FieldType, FieldTypeKind, StructKind};
use crate::codes;
use crate::load::ThriftFile;

/// A definition: the file it is in and its index among the file's
/// definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId {
    pub file: usize,
    pub index: usize,
}

/// What [`link`] found.
#[derive(Debug, Default)]
pub struct Linked {
    /// Per file, its definitions by name.
    tables: Vec<HashMap<String, usize>>,
    /// Per file, the files it includes by prefix.
    prefixes: Vec<HashMap<String, usize>>,
    pub diagnostics: Diagnostics,
}

impl Linked {
    /// The definition `name` refers to from `file`.
    pub fn resolve(&self, file: usize, name: &str) -> Option<DefId> {
        if let Some(&index) = self.tables.get(file)?.get(name) {
            return Some(DefId { file, index });
        }
        let (prefix, rest) = name.split_once('.')?;
        let included = *self.prefixes[file].get(prefix)?;
        let &index = self.tables[included].get(rest)?;
        Some(DefId {
            file: included,
            index,
        })
    }
}

/// The largest field id: ids are 16-bit.
pub const MAX_FIELD_ID: i64 = i16::MAX as i64;

/// Links `files`, reporting what is wrong with them.
pub fn link(files: &[ThriftFile]) -> Linked {
    let mut linker = Linker {
        files,
        linked: Linked::default(),
    };
    for file in files {
        linker.table(file);
    }
    for (index, file) in files.iter().enumerate() {
        linker.check_file(index, file);
    }
    linker.linked
}

struct Linker<'a> {
    files: &'a [ThriftFile],
    linked: Linked,
}

impl Linker<'_> {
    fn error(&mut self, code: faultline_source::DiagnosticCode, message: String, span: Span) {
        self.linked.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    fn table(&mut self, file: &ThriftFile) {
        let mut table = HashMap::new();
        for (position, definition) in file.document.definitions.iter().enumerate() {
            let name = definition.name();
            if table.insert(name.text.clone(), position).is_some() {
                self.error(
                    codes::DUPLICATE_DEFINITION,
                    format!("{} is defined twice", name.text),
                    name.span,
                );
            }
        }
        let prefixes = file
            .includes
            .iter()
            .flatten()
            .map(|&included| (self.files[included].prefix.clone(), included))
            .collect();
        self.linked.tables.push(table);
        self.linked.prefixes.push(prefixes);
    }

    fn check_file(&mut self, file: usize, thrift: &ThriftFile) {
        for (index, definition) in thrift.document.definitions.iter().enumerate() {
            match definition {
                Definition::Const(c) => self.check_type(file, &c.ty),
                Definition::Typedef(t) => {
                    self.check_type(file, &t.ty);
                    self.check_typedef_cycle(DefId { file, index }, t.name.span);
                }
                Definition::Enum(e) => self.check_enum(e),
                Definition::Struct(s) => {
                    let what = s.kind.keyword();
                    self.check_fields(file, &s.fields, &format!("{what} {}", s.name.text));
                }
                Definition::Service(service) => {
                    if let Some(parent) = &service.extends {
                        self.check_extends(file, index, parent.text.as_str(), parent.span);
                    }
                    let mut names = HashSet::new();
                    for function in &service.functions {
                        if !names.insert(function.name.text.as_str()) {
                            self.error(
                                codes::DUPLICATE_DEFINITION,
                                format!(
                                    "function {} is defined twice in service {}",
                                    function.name.text, service.name.text
                                ),
                                function.name.span,
                            );
                        }
                        if let Some(returns) = &function.returns {
                            self.check_type(file, returns);
                        }
                        let name = &function.name.text;
                        self.check_fields(
                            file,
                            &function.arguments,
                            &format!("{name}'s arguments"),
                        );
                        self.check_fields(file, &function.throws, &format!("{name}'s exceptions"));
                        for thrown in &function.throws {
                            self.check_exception(file, &thrown.ty);
                        }
                        if function.oneway
                            && (function.returns.is_some() || !function.throws.is_empty())
                        {
                            self.error(
                                codes::WRONG_KIND,
                                format!(
                                    "oneway function {name} can neither return a value nor throw"
                                ),
                                function.name.span,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Value names are unique and numbers fit in an `i32`. A repeated
    /// number is allowed, but warned about: the two names cannot be told
    /// apart on the wire.
    fn check_enum(&mut self, e: &crate::ast::Enum) {
        let mut names = HashSet::new();
        let mut numbers = HashMap::new();
        let mut next = 0i64;
        for value in &e.values {
            let number = value.value.map_or(next, |(n, _)| n);
            next = number.saturating_add(1);
            let span = value.value.map_or(value.name.span, |(_, span)| span);
            if !names.insert(value.name.text.as_str()) {
                self.error(
                    codes::DUPLICATE_DEFINITION,
                    format!(
                        "{} is defined twice in enum {}",
                        value.name.text, e.name.text
                    ),
                    value.name.span,
                );
            }
            if i32::try_from(number).is_err() {
                self.error(
                    codes::INVALID_FIELD,
                    format!("enum value {number} does not fit in an i32"),
                    span,
                );
            } else if let Some(first) = numbers.insert(number, value.name.text.as_str()) {
                self.linked.diagnostics.push(
                    Diagnostic::warning(format!(
                        "{} has the same number, {number}, as {first}",
                        value.name.text
                    ))
                    .with_code(codes::INVALID_FIELD)
                    .with_primary(span, ""),
                );
            }
        }
    }

    /// Field ids and names are unique, ids in range, and types resolve.
    fn check_fields(&mut self, file: usize, fields: &[Field], owner: &str) {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for field in fields {
            match field.id {
                Some((id, span)) if !(1..=MAX_FIELD_ID).contains(&id) => self.error(
                    codes::INVALID_FIELD,
                    format!("field id {id} is outside 1 to {MAX_FIELD_ID}"),
                    span,
                ),
                Some((id, span)) if !ids.insert(id) => self.error(
                    codes::INVALID_FIELD,
                    format!("field id {id} is used twice in {owner}"),
                    span,
                ),
                Some(_) => {}
                None => self.linked.diagnostics.push(
                    Diagnostic::warning(format!(
                        "field {} has no id; the compiler assigns one that changes when fields are added or reordered",
                        field.name.text
                    ))
                    .with_code(codes::IMPLICIT_ID)
                    .with_primary(field.name.span, ""),
                ),
            }
            if !names.insert(field.name.text.clone()) {
                self.error(
                    codes::INVALID_FIELD,
                    format!("field {} is declared twice in {owner}", field.name.text),
                    field.name.span,
                );
            }
            self.check_type(file, &field.ty);
        }
    }

    /// Every name in `ty` resolves to a type.
    fn check_type(&mut self, file: usize, ty: &FieldType) {
        match &ty.kind {
            FieldTypeKind::Base(_) => {}
            FieldTypeKind::Named(name) => match self.linked.resolve(file, name) {
                None => self.error(
                    codes::UNRESOLVED,
                    format!("type {name} is not defined"),
                    ty.span,
                ),
                Some(id) => {
                    let what = match self.definition(id) {
                        Definition::Service(_) => Some("a service"),
                        Definition::Const(_) => Some("a constant"),
                        _ => None,
                    };
                    if let Some(what) = what {
                        self.error(
                            codes::WRONG_KIND,
                            format!("{name} is {what}, not a type"),
                            ty.span,
                        );
                    }
                }
            },
            FieldTypeKind::List(item) | FieldTypeKind::Set(item) => self.check_type(file, item),
            FieldTypeKind::Map(key, value) => {
                self.check_type(file, key);
                self.check_type(file, value);
            }
        }
    }

    fn definition(&self, id: DefId) -> &Definition {
        &self.files[id.file].document.definitions[id.index]
    }

    /// A thrown type is an exception, possibly through typedefs.
    fn check_exception(&mut self, file: usize, ty: &FieldType) {
        let FieldTypeKind::Named(name) = &ty.kind else {
            self.error(
                codes::WRONG_KIND,
                "only exceptions can be thrown".into(),
                ty.span,
            );
            return;
        };
        let Some(mut id) = self.linked.resolve(file, name) else {
            return;
        };
        for _ in 0..=self.files.len() * 64 {
            match self.definition(id) {
                Definition::Struct(s) if s.kind == StructKind::Exception => return,
                Definition::Typedef(t) => match &t.ty.kind {
                    FieldTypeKind::Named(next) => match self.linked.resolve(id.file, next) {
                        Some(next) => id = next,
                        None => return,
                    },
                    _ => break,
                },
                _ => break,
            }
        }
        self.error(
            codes::WRONG_KIND,
            format!("{name} is not an exception, so it cannot be thrown"),
            ty.span,
        );
    }

    /// Typedefs do not name themselves, directly or through others.
    fn check_typedef_cycle(&mut self, start: DefId, span: Span) {
        let mut seen = HashSet::from([start]);
        let mut id = start;
        loop {
            let Definition::Typedef(t) = self.definition(id) else {
                return;
            };
            let FieldTypeKind::Named(next) = &t.ty.kind else {
                return;
            };
            let Some(next) = self.linked.resolve(id.file, next) else {
                return;
            };
            if next == start {
                let name = &self.definition(start).name().text;
                self.error(
                    codes::CYCLE,
                    format!("typedef {name} refers to itself"),
                    span,
                );
                return;
            }
            if !seen.insert(next) {
                // A cycle that does not pass through `start`; reported
                // at its own typedefs.
                return;
            }
            id = next;
        }
    }

    /// A service extends a service, and not itself.
    fn check_extends(&mut self, file: usize, index: usize, parent: &str, span: Span) {
        let start = DefId { file, index };
        let Some(mut id) = self.linked.resolve(file, parent) else {
            self.error(
                codes::UNRESOLVED,
                format!("service {parent} is not defined"),
                span,
            );
            return;
        };
        let mut seen = HashSet::new();
        loop {
            let Definition::Service(service) = self.definition(id) else {
                self.error(
                    codes::WRONG_KIND,
                    format!("{parent} is not a service"),
                    span,
                );
                return;
            };
            if id == start {
                self.error(codes::CYCLE, "the service extends itself".into(), span);
                return;
            }
            if !seen.insert(id) {
                return;
            }
            let Some(next) = service
                .extends
                .as_ref()
                .and_then(|p| self.linked.resolve(id.file, &p.text))
            else {
                return;
            };
            id = next;
        }
    }
}
