use faultline_source::{SourceMap, SourceName, Span};

/// A resolved source position: file and one-based line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Workspace-relative path with `/` separators, or another display name.
    pub file: String,
    /// The git revision `file` was read from; `None` for the working tree.
    pub revision: Option<String>,
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
    /// Where `span` is. Text embedded in another file (a Protobuf payload in
    /// an AsyncAPI document) is located at the string that holds it, since
    /// the integrations reports feed can only point at files.
    pub fn resolve(sources: &SourceMap, span: Span) -> Option<Location> {
        let span = sources.outermost(span);
        let file = sources.get(span.source())?;
        let text = file.text();
        let lines = file.lines();
        let start = lines.line_col_chars(span.start(), text);
        let end = lines.line_col_chars(span.end(), text);
        let start16 = lines.line_col_utf16(span.start(), text);
        let end16 = lines.line_col_utf16(span.end(), text);
        let (name, revision) = match file.name() {
            SourceName::Path(path) => (
                path.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
                None,
            ),
            SourceName::Revision { rev, path } => (path.clone(), Some(rev.clone())),
            other => (other.to_string(), None),
        };
        Some(Location {
            file: name,
            revision,
            line: start.line + 1,
            column: start.col + 1,
            end_line: end.line + 1,
            end_column: end.col + 1,
            utf16_column: start16.col + 1,
            utf16_end_column: end16.col + 1,
        })
    }

    /// The location to annotate for a change, for integrations that show
    /// annotations on the checked-out files: the first of `spans` in the
    /// working tree, since line numbers in an older revision would point at
    /// the wrong lines, and otherwise the first that resolves at all.
    pub fn for_annotation(sources: &SourceMap, spans: &[Option<Span>]) -> Option<Location> {
        let resolved: Vec<Location> = spans
            .iter()
            .flatten()
            .filter_map(|s| Location::resolve(sources, *s))
            .collect();
        let worktree = resolved.iter().position(|l| l.revision.is_none());
        resolved.into_iter().nth(worktree.unwrap_or(0))
    }
}
