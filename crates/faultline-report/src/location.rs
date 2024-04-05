use faultline_source::{SourceMap, SourceName, Span};

/// A resolved source position: file and one-based line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Workspace-relative path with `/` separators, or another display name.
    pub file: String,
    pub line: u32,
    /// One-based column in Unicode scalar values.
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    /// One-based columns in UTF-16 code units, as SARIF expects by default.
    pub utf16_column: u32,
    pub utf16_end_column: u32,
}

impl Location {
    pub fn resolve(sources: &SourceMap, span: Span) -> Option<Location> {
        let file = sources.get(span.source())?;
        let text = file.text();
        let lines = file.lines();
        let start = lines.line_col_chars(span.start(), text);
        let end = lines.line_col_chars(span.end(), text);
        let start16 = lines.line_col_utf16(span.start(), text);
        let end16 = lines.line_col_utf16(span.end(), text);
        let name = match file.name() {
            SourceName::Path(path) => path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            other => other.to_string(),
        };
        Some(Location {
            file: name,
            line: start.line + 1,
            column: start.col + 1,
            end_line: end.line + 1,
            end_column: end.col + 1,
            utf16_column: start16.col + 1,
            utf16_end_column: end16.col + 1,
        })
    }
}
