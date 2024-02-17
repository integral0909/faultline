use std::fmt;

/// Resource ceilings applied while loading untrusted schema input.
///
/// Schemas arrive from pull requests, so every stage that can be made to do
/// unbounded work by its input has a ceiling here. The defaults are generous
/// enough for the largest real-world API descriptions we know of while keeping
/// memory and time bounded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of a single source file in bytes.
    pub max_file_bytes: u64,
    /// Maximum nesting depth of arrays/objects/collections in one document.
    pub max_depth: u32,
    /// Maximum number of nodes produced when parsing one document, counting
    /// every copy made by YAML alias expansion. A node costs about 90 bytes
    /// plus roughly 130 per object member, so the default bounds one document
    /// tree to a few hundred megabytes in the worst case; the largest public
    /// API descriptions we know of have a few hundred thousand nodes.
    pub max_document_nodes: u64,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_file_bytes: 64 * 1024 * 1024,
        max_depth: 256,
        max_document_nodes: 2_000_000,
    };

    pub fn check_file_size(&self, bytes: u64) -> Result<(), LimitExceeded> {
        LimitKind::FileBytes.check(bytes, self.max_file_bytes)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Which limit was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitKind {
    FileBytes,
    Depth,
    DocumentNodes,
}

impl LimitKind {
    /// Returns an error if `actual` exceeds `limit`.
    pub fn check(self, actual: u64, limit: u64) -> Result<(), LimitExceeded> {
        if actual > limit {
            Err(LimitExceeded { kind: self, limit })
        } else {
            Ok(())
        }
    }

    fn describe(self) -> &'static str {
        match self {
            LimitKind::FileBytes => "file size in bytes",
            LimitKind::Depth => "nesting depth",
            LimitKind::DocumentNodes => "number of nodes in a document",
        }
    }
}

/// Input exceeded one of the configured [`Limits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitExceeded {
    pub kind: LimitKind,
    pub limit: u64,
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} exceeds the limit of {}",
            self.kind.describe(),
            self.limit
        )
    }
}

impl std::error::Error for LimitExceeded {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_inclusive() {
        let limits = Limits {
            max_file_bytes: 10,
            ..Limits::DEFAULT
        };
        assert!(limits.check_file_size(10).is_ok());
        let err = limits.check_file_size(11).unwrap_err();
        assert_eq!(err.kind, LimitKind::FileBytes);
        assert_eq!(
            err.to_string(),
            "file size in bytes exceeds the limit of 10"
        );
    }
}
