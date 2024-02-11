use std::ops::Range;

/// A zero-based line and column.
///
/// The unit of `col` depends on which [`LineIndex`] method produced it: bytes,
/// Unicode scalar values, or UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Maps byte offsets to line/column positions and back.
///
/// `\n`, `\r\n` and a lone `\r` all terminate a line. JSON only treats them as
/// whitespace, but YAML 1.2 recognises all three as line breaks, and reporting
/// positions consistently across formats matters more than matching one
/// grammar exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of the first character of each line. Always starts with 0.
    line_starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    /// Indexes `text`. The caller guarantees `text.len()` fits in `u32`, which
    /// [`SourceMap`](crate::SourceMap) enforces before constructing files.
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut line_starts = vec![0];
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                    line_starts.push((i + 2) as u32);
                    i += 1;
                }
                b'\n' | b'\r' => line_starts.push((i + 1) as u32),
                _ => {}
            }
            i += 1;
        }
        Self {
            line_starts,
            len: text.len() as u32,
        }
    }

    /// Number of lines. A file that ends with a line break has a final, empty
    /// line after it, matching how editors number lines.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The zero-based line containing `offset`. Offsets past the end clamp to
    /// the last line.
    pub fn line_of(&self, offset: u32) -> u32 {
        let offset = offset.min(self.len);
        // partition_point returns the number of starts <= offset; line_starts[0]
        // is 0 so the result is always at least 1.
        (self.line_starts.partition_point(|&start| start <= offset) - 1) as u32
    }

    /// Byte range of `line` including its terminator, or `None` if the line
    /// does not exist.
    pub fn line_range_with_terminator(&self, line: u32) -> Option<Range<u32>> {
        let start = *self.line_starts.get(line as usize)?;
        let end = self
            .line_starts
            .get(line as usize + 1)
            .copied()
            .unwrap_or(self.len);
        Some(start..end)
    }

    /// Byte range of `line` without its terminator.
    pub fn line_range(&self, line: u32, text: &str) -> Option<Range<u32>> {
        let range = self.line_range_with_terminator(line)?;
        let content = &text.as_bytes()[range.start as usize..range.end as usize];
        let trimmed = match content {
            [.., b'\r', b'\n'] => 2,
            [.., b'\n' | b'\r'] => 1,
            _ => 0,
        };
        Some(range.start..range.end - trimmed)
    }

    /// Position of `offset` with the column measured in bytes.
    pub fn line_col(&self, offset: u32) -> LineCol {
        let offset = offset.min(self.len);
        let line = self.line_of(offset);
        LineCol {
            line,
            col: offset - self.line_starts[line as usize],
        }
    }

    /// Position of `offset` with the column measured in Unicode scalar values.
    /// This is what humans expect in terminal output.
    ///
    /// An offset inside a multi-byte character is rounded down to that
    /// character's start.
    pub fn line_col_chars(&self, offset: u32, text: &str) -> LineCol {
        self.line_col_with(offset, text, |s| s.chars().count())
    }

    /// Position of `offset` with the column measured in UTF-16 code units, as
    /// required by the Language Server Protocol's default position encoding.
    pub fn line_col_utf16(&self, offset: u32, text: &str) -> LineCol {
        self.line_col_with(offset, text, |s| s.encode_utf16().count())
    }

    fn line_col_with(&self, offset: u32, text: &str, measure: impl Fn(&str) -> usize) -> LineCol {
        let offset = floor_char_boundary(text, offset.min(self.len));
        let line = self.line_of(offset);
        let start = self.line_starts[line as usize];
        let prefix = &text[start as usize..offset as usize];
        LineCol {
            line,
            col: measure(prefix) as u32,
        }
    }

    /// Converts a UTF-16 based position back to a byte offset.
    ///
    /// Columns past the end of the line clamp to the end of the line (before
    /// its terminator), which is how LSP clients expect out-of-range columns to
    /// be treated. Returns `None` only for a line that does not exist.
    pub fn offset_utf16(&self, pos: LineCol, text: &str) -> Option<u32> {
        let range = self.line_range(pos.line, text)?;
        let line = &text[range.start as usize..range.end as usize];
        let mut units = 0u32;
        for (byte_idx, ch) in line.char_indices() {
            if units >= pos.col {
                return Some(range.start + byte_idx as u32);
            }
            units += ch.len_utf16() as u32;
        }
        Some(range.end)
    }
}

fn floor_char_boundary(text: &str, mut offset: u32) -> u32 {
    while offset > 0 && !text.is_char_boundary(offset as usize) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_text_has_one_line() {
        let idx = LineIndex::new("");
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.line_col(0), LineCol { line: 0, col: 0 });
    }

    #[test]
    fn all_line_terminators_are_recognised() {
        let text = "a\nb\r\nc\rd";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 4);
        assert_eq!(idx.line_col(7), LineCol { line: 3, col: 0 });
        assert_eq!(idx.line_range(1, text), Some(2..3));
        assert_eq!(idx.line_range_with_terminator(1), Some(2..5));
        assert_eq!(idx.line_range(2, text), Some(5..6));
    }

    #[test]
    fn crlf_is_a_single_break() {
        let idx = LineIndex::new("x\r\n\r\ny");
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.line_of(5), 2);
    }

    #[test]
    fn trailing_newline_opens_an_empty_final_line() {
        let text = "a\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 2);
        assert_eq!(idx.line_range(1, text), Some(2..2));
        assert_eq!(idx.line_range(2, text), None);
    }

    #[test]
    fn columns_in_each_unit() {
        // 'é' is 2 bytes / 1 char / 1 UTF-16 unit; '𝄞' is 4 bytes / 1 char / 2 units.
        let text = "é𝄞x";
        let idx = LineIndex::new(text);
        let x = text.find('x').unwrap() as u32;
        assert_eq!(idx.line_col(x).col, 6);
        assert_eq!(idx.line_col_chars(x, text).col, 2);
        assert_eq!(idx.line_col_utf16(x, text).col, 3);
    }

    #[test]
    fn offsets_inside_a_character_round_down() {
        let text = "a𝄞";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col_chars(3, text).col, 1);
    }

    #[test]
    fn offsets_past_the_end_clamp() {
        let idx = LineIndex::new("ab\ncd");
        assert_eq!(idx.line_col(100), LineCol { line: 1, col: 2 });
    }

    #[test]
    fn utf16_column_past_line_end_clamps_before_terminator() {
        let text = "ab\r\ncd";
        let idx = LineIndex::new(text);
        assert_eq!(
            idx.offset_utf16(LineCol { line: 0, col: 99 }, text),
            Some(2)
        );
        assert_eq!(idx.offset_utf16(LineCol { line: 5, col: 0 }, text), None);
    }

    proptest! {
        #[test]
        fn utf16_positions_round_trip(text in "(\\PC|\r|\n){0,64}", pick in any::<prop::sample::Index>()) {
            let idx = LineIndex::new(&text);
            let boundaries: Vec<u32> = text
                .char_indices()
                .map(|(i, _)| i as u32)
                .chain(std::iter::once(text.len() as u32))
                // Offsets between '\r' and '\n' are not addressable as a line/column.
                .filter(|&i| !(i > 0 && text.as_bytes()[i as usize - 1] == b'\r'
                    && text.as_bytes().get(i as usize) == Some(&b'\n')))
                .collect();
            let offset = boundaries[pick.index(boundaries.len())];
            let pos = idx.line_col_utf16(offset, &text);
            prop_assert_eq!(idx.offset_utf16(pos, &text), Some(offset));
        }

        #[test]
        fn line_of_is_monotonic(text in "(\\PC|\r|\n){0,64}") {
            let idx = LineIndex::new(&text);
            let mut prev = 0;
            for offset in 0..=text.len() as u32 {
                let line = idx.line_of(offset);
                prop_assert!(line >= prev);
                prev = line;
            }
            prop_assert_eq!(prev + 1, idx.line_count());
        }
    }
}
