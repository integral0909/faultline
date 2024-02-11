use std::cmp::Ordering;
use std::fmt;

use crate::span::Span;

/// How serious a diagnostic is.
///
/// Ordered from most to least severe so that sorting puts errors first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stable identifier for a class of input diagnostic, e.g. `FLE0101`.
///
/// Codes are part of Faultline's public interface: users search for them and
/// tooling filters on them, so a code is never reused for a different meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticCode(&'static str);

impl DiagnosticCode {
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A span annotated with an optional message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: Option<String>,
    /// Primary labels mark where the problem is; secondary labels add context
    /// such as "first defined here".
    pub primary: bool,
}

/// A problem found in user input, with enough structure to render it with
/// source context or to serialize it for machines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<DiagnosticCode>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    #[must_use]
    pub fn with_code(mut self, code: DiagnosticCode) -> Self {
        self.code = Some(code);
        self
    }

    #[must_use]
    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: non_empty(message.into()),
            primary: true,
        });
        self
    }

    #[must_use]
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: non_empty(message.into()),
            primary: false,
        });
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// The first primary label's span, falling back to the first label.
    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span)
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Total order used to make diagnostic output deterministic regardless of
    /// the order in which parallel front ends produced them.
    fn sort_key_cmp(&self, other: &Self) -> Ordering {
        let pos = |d: &Self| d.primary_span().map(|s| (s.source(), s.start(), s.end()));
        pos(self)
            .cmp(&pos(other))
            .then(self.severity.cmp(&other.severity))
            .then(self.code.cmp(&other.code))
            .then_with(|| self.message.cmp(&other.message))
    }
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// An ordered collection of diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.is_error()).count()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }

    /// Sorts into the canonical order: by file and position, then severity.
    pub fn sort(&mut self) {
        self.items.sort_by(Diagnostic::sort_key_cmp);
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
}

impl Extend<Diagnostic> for Diagnostics {
    fn extend<T: IntoIterator<Item = Diagnostic>>(&mut self, iter: T) {
        self.items.extend(iter);
    }
}

impl FromIterator<Diagnostic> for Diagnostics {
    fn from_iter<T: IntoIterator<Item = Diagnostic>>(iter: T) -> Self {
        Self {
            items: iter.into_iter().collect(),
        }
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a> IntoIterator for &'a Diagnostics {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::SourceId;

    fn at(file: u32, start: u32) -> Span {
        Span::new(SourceId::from_raw(file), start, start + 1)
    }

    #[test]
    fn empty_label_messages_are_dropped() {
        let d = Diagnostic::error("x").with_primary(at(0, 0), "");
        assert_eq!(d.labels[0].message, None);
    }

    #[test]
    fn primary_span_prefers_primary_labels() {
        let d = Diagnostic::error("x")
            .with_secondary(at(0, 1), "context")
            .with_primary(at(0, 9), "here");
        assert_eq!(d.primary_span(), Some(at(0, 9)));
    }

    #[test]
    fn sorting_is_by_position_then_severity() {
        let mut ds: Diagnostics = [
            Diagnostic::warning("w").with_primary(at(0, 5), ""),
            Diagnostic::error("late file").with_primary(at(1, 0), ""),
            Diagnostic::error("e").with_primary(at(0, 5), ""),
            Diagnostic::error("first").with_primary(at(0, 1), ""),
            Diagnostic::error("no span"),
        ]
        .into_iter()
        .collect();
        ds.sort();
        let order: Vec<_> = ds.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(order, ["no span", "first", "e", "w", "late file"]);
        assert_eq!(ds.error_count(), 4);
    }
}
