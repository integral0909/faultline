//! Plain-text rendering of [`Diagnostic`]s with source excerpts.
//!
//! The layout follows the convention most compilers have settled on:
//!
//! ```text
//! error[FLE0101]: unexpected character '}'
//!   --> api/openapi.yaml:12:7
//!    |
//! 12 |   foo: }
//!    |        ^ expected a value
//!    |
//!    = help: remove the brace
//! ```
//!
//! Output is uncolored and deterministic so it can be snapshot-tested and
//! embedded in other formats; terminal styling is applied by the CLI.

use std::fmt::Write as _;

use crate::diagnostic::{Diagnostic, Label};
use crate::file::SourceFile;
use crate::map::SourceMap;
use crate::span::{SourceId, Span};

const TAB_WIDTH: usize = 4;

/// Renders `diagnostic` to a string, resolving spans through `map`.
///
/// Labels whose file is unknown to `map` are skipped rather than causing an
/// error: a diagnostic should always be printable, even if degraded.
pub fn render(diagnostic: &Diagnostic, map: &SourceMap) -> String {
    let mut out = String::new();
    write_header(&mut out, diagnostic);

    let groups = group_labels_by_file(diagnostic);
    let gutter = groups
        .iter()
        .filter_map(|(id, labels)| {
            let file = map.get(*id)?;
            labels
                .iter()
                .map(|l| file.lines().line_of(l.span.start()) + 1)
                .max()
        })
        .max()
        .map_or(1, |n| n.to_string().len());

    for (id, labels) in &groups {
        if let Some(file) = map.get(*id) {
            write_file_section(&mut out, &file, labels, gutter);
        }
    }

    if !diagnostic.notes.is_empty() || diagnostic.help.is_some() {
        if !groups.is_empty() {
            let _ = writeln!(out, "{:gutter$} |", "");
        }
        for note in &diagnostic.notes {
            let _ = writeln!(out, "{:gutter$} = note: {note}", "");
        }
        if let Some(help) = &diagnostic.help {
            let _ = writeln!(out, "{:gutter$} = help: {help}", "");
        }
    }
    out
}

fn write_header(out: &mut String, diagnostic: &Diagnostic) {
    out.push_str(diagnostic.severity.as_str());
    if let Some(code) = diagnostic.code {
        let _ = write!(out, "[{code}]");
    }
    let _ = writeln!(out, ": {}", diagnostic.message);
}

/// Groups labels by file, with the file of the primary label first and the
/// remaining files in order of first appearance.
fn group_labels_by_file(diagnostic: &Diagnostic) -> Vec<(SourceId, Vec<&Label>)> {
    let mut groups: Vec<(SourceId, Vec<&Label>)> = Vec::new();
    let primary_file = diagnostic.primary_span().map(Span::source);
    for label in &diagnostic.labels {
        let id = label.span.source();
        match groups.iter_mut().find(|(g, _)| *g == id) {
            Some((_, labels)) => labels.push(label),
            None => groups.push((id, vec![label])),
        }
    }
    if let Some(primary) = primary_file
        && let Some(pos) = groups.iter().position(|(id, _)| *id == primary)
    {
        let group = groups.remove(pos);
        groups.insert(0, group);
    }
    groups
}

fn write_file_section(out: &mut String, file: &SourceFile, labels: &[&Label], gutter: usize) {
    let text = file.text();
    let lines = file.lines();
    let anchor = labels.iter().find(|l| l.primary).unwrap_or(&labels[0]).span;
    let pos = lines.line_col_chars(anchor.start(), text);
    let _ = writeln!(
        out,
        "{:gutter$}--> {}:{}:{}",
        "",
        file.name(),
        pos.line + 1,
        pos.col + 1
    );
    let _ = writeln!(out, "{:gutter$} |", "");

    let mut by_line: Vec<(u32, &Label)> = labels
        .iter()
        .map(|l| (lines.line_of(l.span.start()), *l))
        .collect();
    by_line.sort_by_key(|(line, l)| (*line, l.span.start(), !l.primary));

    let mut previous_line: Option<u32> = None;
    let mut i = 0;
    while i < by_line.len() {
        let line = by_line[i].0;
        if let Some(prev) = previous_line
            && line > prev + 1
        {
            out.push_str("...\n");
        }
        let Some(range) = lines.line_range(line, text) else {
            i += 1;
            continue;
        };
        let line_text = &text[range.start as usize..range.end as usize];
        let _ = writeln!(out, "{:>gutter$} | {}", line + 1, expand_tabs(line_text));

        while i < by_line.len() && by_line[i].0 == line {
            let label = by_line[i].1;
            write_underline(
                out,
                label,
                range.start,
                line_text,
                lines.line_of(label.span.end()),
                gutter,
            );
            i += 1;
        }
        previous_line = Some(line);
    }
}

fn write_underline(
    out: &mut String,
    label: &Label,
    line_start: u32,
    line_text: &str,
    end_line: u32,
    gutter: usize,
) {
    let start_in_line = (label.span.start() - line_start) as usize;
    let start_in_line = start_in_line.min(line_text.len());
    let multiline = label.span.end() > line_start + line_text.len() as u32;
    let end_in_line = if multiline {
        line_text.len()
    } else {
        (label.span.end() - line_start) as usize
    };

    let pad = display_width(prefix_until(line_text, start_in_line));
    let width = display_width(slice_between(line_text, start_in_line, end_in_line)).max(1);
    let marker = if label.primary { '^' } else { '-' };

    let _ = write!(out, "{:gutter$} | {:pad$}", "", "");
    for _ in 0..width {
        out.push(marker);
    }
    let mut message = label.message.clone().unwrap_or_default();
    if multiline {
        let suffix = format!("(continues to line {})", end_line + 1);
        if message.is_empty() {
            message = suffix;
        } else {
            message = format!("{message} {suffix}");
        }
    }
    if !message.is_empty() {
        let _ = write!(out, " {message}");
    }
    out.push('\n');
}

fn prefix_until(text: &str, byte: usize) -> &str {
    let mut end = byte.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn slice_between(text: &str, start: usize, end: usize) -> &str {
    let start = prefix_until(text, start).len();
    let end = prefix_until(text, end.max(start)).len();
    &text[start..end]
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, DiagnosticCode};
    use crate::file::SourceName;

    fn map_with(name: &str, text: &str) -> (SourceMap, SourceId) {
        let map = SourceMap::new();
        let id = map.add(SourceName::Path(name.into()), text).unwrap().id();
        (map, id)
    }

    #[test]
    fn single_label_with_code_note_and_help() {
        let (map, id) = map_with("api.yaml", "openapi: 3.1.0\npaths:\n  foo: }\n");
        let d = Diagnostic::error("unexpected character '}'")
            .with_code(DiagnosticCode::new("FLE0101"))
            .with_primary(Span::new(id, 29, 30), "expected a value")
            .with_note("flow mappings must be opened with '{'")
            .with_help("remove the brace");
        let expected = "\
error[FLE0101]: unexpected character '}'
 --> api.yaml:3:8
  |
3 |   foo: }
  |        ^ expected a value
  |
  = note: flow mappings must be opened with '{'
  = help: remove the brace
";
        assert_eq!(render(&d, &map), expected);
    }

    #[test]
    fn secondary_labels_elide_gaps_and_widen_gutter() {
        let text = (1..=12).fold(String::new(), |mut acc, n| {
            let _ = writeln!(acc, "line{n}");
            acc
        });
        let (map, id) = map_with("f.json", &text);
        let line = |n: u32| {
            let start = text
                .match_indices('\n')
                .nth(n as usize - 2)
                .map_or(0, |(i, _)| i + 1) as u32;
            Span::new(id, start, start + 4)
        };
        let d = Diagnostic::error("duplicate key")
            .with_primary(line(11), "duplicate")
            .with_secondary(line(2), "first defined here");
        let expected = "\
error: duplicate key
  --> f.json:11:1
   |
 2 | line2
   | ---- first defined here
...
11 | line11
   | ^^^^ duplicate
";
        assert_eq!(render(&d, &map), expected);
    }

    #[test]
    fn multiline_spans_underline_to_end_of_first_line() {
        let (map, id) = map_with("x.yaml", "a: [1,\n  2]\n");
        let d = Diagnostic::error("bad list").with_primary(Span::new(id, 3, 11), "");
        let rendered = render(&d, &map);
        assert!(
            rendered.contains("1 | a: [1,\n  |    ^^^ (continues to line 2)\n"),
            "{rendered}"
        );
    }

    #[test]
    fn tabs_and_wide_characters_align() {
        let (map, id) = map_with("t", "\té: x\n");
        let x = "\té: ".len() as u32;
        let d = Diagnostic::error("e").with_primary(Span::empty(id, x), "here");
        let rendered = render(&d, &map);
        assert!(
            rendered.contains("1 |     é: x\n  |        ^ here\n"),
            "{rendered}"
        );
    }

    #[test]
    fn labels_in_unknown_files_are_skipped() {
        let map = SourceMap::new();
        let d = Diagnostic::error("orphan")
            .with_primary(Span::new(SourceId::from_raw(3), 0, 1), "x")
            .with_help("h");
        assert_eq!(render(&d, &map), "error: orphan\n  |\n  = help: h\n");
    }
}
