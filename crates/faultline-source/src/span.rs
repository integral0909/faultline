use std::fmt;
use std::ops::Range;

/// Identifies a file registered in a [`SourceMap`](crate::SourceMap).
///
/// Ids are dense indices assigned in registration order and are only
/// meaningful relative to the map that issued them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u32);

impl SourceId {
    /// Builds an id from a raw index. Intended for maps and tests; ids that were
    /// not issued by a map will simply fail to resolve.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn as_raw(self) -> u32 {
        self.0
    }

    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Debug for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SourceId({})", self.0)
    }
}

/// A half-open byte range `[start, end)` within a single source file.
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    source: SourceId,
    start: u32,
    end: u32,
}

impl Span {
    /// Creates a span. `start` must not exceed `end`.
    pub fn new(source: SourceId, start: u32, end: u32) -> Self {
        assert!(start <= end, "span start {start} is after end {end}");
        Self { source, start, end }
    }

    /// An empty span positioned at `offset`, used for "expected X here" errors.
    pub fn empty(source: SourceId, offset: u32) -> Self {
        Self::new(source, offset, offset)
    }

    pub const fn source(self) -> SourceId {
        self.source
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }

    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub const fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Returns `true` if `other` lies entirely within `self`.
    pub fn contains(self, other: Span) -> bool {
        self.source == other.source && self.start <= other.start && other.end <= self.end
    }

    /// Returns `true` if `offset` lies within `[start, end)`.
    pub fn contains_offset(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// The smallest span covering both `self` and `other`.
    ///
    /// Returns `None` when the spans belong to different files, since no single
    /// range can describe them.
    pub fn join(self, other: Span) -> Option<Span> {
        (self.source == other.source).then(|| Span {
            source: self.source,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        })
    }

    /// The empty span at the start of `self`.
    pub fn shrink_to_start(self) -> Span {
        Span::empty(self.source, self.start)
    }

    /// The empty span at the end of `self`.
    pub fn shrink_to_end(self) -> Span {
        Span::empty(self.source, self.end)
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}..{}", self.source.0, self.start, self.end)
    }
}

/// A value paired with the span it was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub const fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            value: f(self.value),
            span: self.span,
        }
    }

    pub const fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            value: &self.value,
            span: self.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SourceId = SourceId::from_raw(0);
    const B: SourceId = SourceId::from_raw(1);

    #[test]
    fn join_covers_both_spans_in_either_order() {
        let x = Span::new(A, 4, 8);
        let y = Span::new(A, 10, 12);
        assert_eq!(x.join(y), Some(Span::new(A, 4, 12)));
        assert_eq!(y.join(x), Some(Span::new(A, 4, 12)));
    }

    #[test]
    fn join_refuses_spans_from_different_files() {
        assert_eq!(Span::new(A, 0, 1).join(Span::new(B, 0, 1)), None);
    }

    #[test]
    fn containment_is_file_aware() {
        let outer = Span::new(A, 0, 10);
        assert!(outer.contains(Span::new(A, 2, 10)));
        assert!(outer.contains(Span::empty(A, 10)));
        assert!(!outer.contains(Span::new(A, 2, 11)));
        assert!(!outer.contains(Span::new(B, 2, 3)));
        assert!(outer.contains_offset(0));
        assert!(!outer.contains_offset(10));
    }

    #[test]
    #[should_panic(expected = "after end")]
    fn inverted_span_is_rejected() {
        let _ = Span::new(A, 5, 4);
    }
}
