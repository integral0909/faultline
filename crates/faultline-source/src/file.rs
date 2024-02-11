use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::line_index::LineIndex;
use crate::span::{SourceId, Span};

/// Where a source file came from, used for display and for deduplication.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceName {
    /// A file on the local filesystem, relative to the workspace root when
    /// possible.
    Path(PathBuf),
    /// A blob read from a git revision. `path` always uses `/` separators.
    Revision { rev: String, path: String },
    /// A document fetched over the network.
    Url(String),
    /// Standard input.
    Stdin,
    /// Text that has no on-disk origin, such as built-in definitions.
    Virtual(String),
}

impl fmt::Display for SourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceName::Path(path) => write!(f, "{}", path.display()),
            SourceName::Revision { rev, path } => write!(f, "{rev}:{path}"),
            SourceName::Url(url) => f.write_str(url),
            SourceName::Stdin => f.write_str("<stdin>"),
            SourceName::Virtual(name) => write!(f, "<{name}>"),
        }
    }
}

/// An immutable, decoded source file.
#[derive(Debug)]
pub struct SourceFile {
    id: SourceId,
    name: SourceName,
    text: Arc<str>,
    lines: LineIndex,
}

impl SourceFile {
    pub(crate) fn new(id: SourceId, name: SourceName, text: Arc<str>) -> Self {
        let lines = LineIndex::new(&text);
        Self {
            id,
            name,
            text,
            lines,
        }
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn name(&self) -> &SourceName {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// A cheap shared handle to the text.
    pub fn text_arc(&self) -> Arc<str> {
        Arc::clone(&self.text)
    }

    pub fn lines(&self) -> &LineIndex {
        &self.lines
    }

    pub fn len(&self) -> u32 {
        self.text.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// A span covering the whole file.
    pub fn full_span(&self) -> Span {
        Span::new(self.id, 0, self.len())
    }

    /// The text covered by `span`, or `None` if the span belongs to another
    /// file or is out of range.
    pub fn slice(&self, span: Span) -> Option<&str> {
        if span.source() != self.id {
            return None;
        }
        self.text.get(span.range())
    }
}

/// Reasons raw bytes could not be turned into source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input starts with a byte-order mark for an encoding other than
    /// UTF-8. JSON (RFC 8259 §8.1) requires UTF-8 for interchange and every
    /// schema ecosystem we support uses it in practice, so rather than guess we
    /// ask the user to re-encode.
    UnsupportedEncoding(&'static str),
    /// The input is not valid UTF-8. `offset` is the first invalid byte in the
    /// raw input, after any BOM.
    InvalidUtf8 { offset: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::UnsupportedEncoding(enc) => {
                write!(f, "file is encoded as {enc}; only UTF-8 is supported")
            }
            DecodeError::InvalidUtf8 { offset } => {
                write!(
                    f,
                    "file is not valid UTF-8 (first invalid byte at offset {offset})"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes raw file contents into text, stripping a UTF-8 byte-order mark.
///
/// Offsets in all spans are relative to the returned text, so a stripped BOM
/// shifts nothing a user can see.
pub fn decode_text(bytes: &[u8]) -> Result<&str, DecodeError> {
    // Order matters: the UTF-32LE BOM begins with the UTF-16LE BOM.
    const BOMS: [(&[u8], &str); 4] = [
        (&[0x00, 0x00, 0xFE, 0xFF], "UTF-32BE"),
        (&[0xFF, 0xFE, 0x00, 0x00], "UTF-32LE"),
        (&[0xFE, 0xFF], "UTF-16BE"),
        (&[0xFF, 0xFE], "UTF-16LE"),
    ];
    let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if body.len() == bytes.len()
        && let Some((_, enc)) = BOMS.iter().find(|(bom, _)| bytes.starts_with(bom))
    {
        return Err(DecodeError::UnsupportedEncoding(enc));
    }
    std::str::from_utf8(body).map_err(|e| DecodeError::InvalidUtf8 {
        offset: e.valid_up_to(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_is_stripped() {
        assert_eq!(decode_text(b"\xEF\xBB\xBF{}"), Ok("{}"));
    }

    #[test]
    fn other_boms_are_rejected_with_their_name() {
        assert_eq!(
            decode_text(b"\xFF\xFE{\x00"),
            Err(DecodeError::UnsupportedEncoding("UTF-16LE"))
        );
        assert_eq!(
            decode_text(b"\xFF\xFE\x00\x00{\x00\x00\x00"),
            Err(DecodeError::UnsupportedEncoding("UTF-32LE"))
        );
        assert_eq!(
            decode_text(b"\xFE\xFF\x00{"),
            Err(DecodeError::UnsupportedEncoding("UTF-16BE"))
        );
    }

    #[test]
    fn invalid_utf8_reports_offset_after_bom() {
        assert_eq!(
            decode_text(b"\xEF\xBB\xBFab\xFFc"),
            Err(DecodeError::InvalidUtf8 { offset: 2 })
        );
    }

    #[test]
    fn source_names_display_like_their_origin() {
        let rev = SourceName::Revision {
            rev: "origin/main".into(),
            path: "api/openapi.yaml".into(),
        };
        assert_eq!(rev.to_string(), "origin/main:api/openapi.yaml");
        assert_eq!(SourceName::Stdin.to_string(), "<stdin>");
        assert_eq!(SourceName::Virtual("wkt".into()).to_string(), "<wkt>");
    }
}
