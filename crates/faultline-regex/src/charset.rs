//! Sets of Unicode scalar values as sorted, disjoint, non-adjacent ranges.

use std::fmt;

/// The largest code point.
pub const MAX: u32 = 0x10_FFFF;

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct CharSet {
    /// Inclusive ranges, sorted, disjoint and not touching.
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn full() -> Self {
        Self {
            ranges: vec![(0, MAX)],
        }
    }

    pub fn single(c: u32) -> Self {
        Self::range(c, c)
    }

    pub fn range(lo: u32, hi: u32) -> Self {
        let mut set = Self::empty();
        set.add(lo, hi);
        set
    }

    pub fn from_ranges(ranges: &[(u32, u32)]) -> Self {
        let mut set = Self::empty();
        for &(lo, hi) in ranges {
            set.add(lo, hi);
        }
        set
    }

    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn contains(&self, c: u32) -> bool {
        self.ranges
            .binary_search_by(|&(lo, hi)| {
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    /// Adds `lo..=hi`, merging with overlapping or adjacent ranges.
    pub fn add(&mut self, lo: u32, hi: u32) {
        let (lo, hi) = (lo.min(hi), hi.max(lo).min(MAX));
        if lo > MAX {
            return;
        }
        let mut merged = (lo, hi);
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut placed = false;
        for &(a, b) in &self.ranges {
            if b.saturating_add(1) < merged.0 {
                out.push((a, b));
            } else if merged.1.saturating_add(1) < a {
                if !placed {
                    out.push(merged);
                    placed = true;
                }
                out.push((a, b));
            } else {
                merged = (merged.0.min(a), merged.1.max(b));
            }
        }
        if !placed {
            out.push(merged);
        }
        self.ranges = out;
    }

    #[must_use]
    pub fn union(&self, other: &CharSet) -> CharSet {
        let mut out = self.clone();
        for &(lo, hi) in &other.ranges {
            out.add(lo, hi);
        }
        out
    }

    /// Every code point not in the set.
    #[must_use]
    pub fn complement(&self) -> CharSet {
        let mut out = Vec::new();
        let mut next = 0u32;
        for &(lo, hi) in &self.ranges {
            if lo > next {
                out.push((next, lo - 1));
            }
            next = hi.saturating_add(1);
        }
        if next <= MAX {
            out.push((next, MAX));
        }
        CharSet { ranges: out }
    }

    #[must_use]
    pub fn intersect(&self, other: &CharSet) -> CharSet {
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a, b) = self.ranges[i];
            let (c, d) = other.ranges[j];
            let (lo, hi) = (a.max(c), b.min(d));
            if lo <= hi {
                out.push((lo, hi));
            }
            if b < d {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges: out }
    }

    /// A readable member, preferring letters and digits, for witnesses.
    pub fn representative(&self) -> Option<char> {
        const PREFERRED: [(u32, u32); 4] = [(0x61, 0x7A), (0x41, 0x5A), (0x30, 0x39), (0x20, 0x7E)];
        for (lo, hi) in PREFERRED {
            let hit = self.intersect(&CharSet::range(lo, hi));
            if let Some(&(first, _)) = hit.ranges.first() {
                return char::from_u32(first);
            }
        }
        self.ranges
            .iter()
            .flat_map(|&(lo, hi)| lo..=hi.min(lo.saturating_add(0x800)))
            .find_map(char::from_u32)
    }
}

impl fmt::Debug for CharSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[")?;
        for &(lo, hi) in &self.ranges {
            if lo == hi {
                write!(f, "{lo:#x} ")?;
            } else {
                write!(f, "{lo:#x}-{hi:#x} ")?;
            }
        }
        f.write_str("]")
    }
}

/// Splits the code points covered by `sets` into the coarsest disjoint
/// segments on which every set is either full or empty.
pub fn partition(sets: &[&CharSet]) -> Vec<CharSet> {
    let mut bounds: Vec<u32> = vec![0];
    for set in sets {
        for &(lo, hi) in set.ranges() {
            bounds.push(lo);
            if hi < MAX {
                bounds.push(hi + 1);
            }
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    let mut segments = Vec::with_capacity(bounds.len());
    for (i, &lo) in bounds.iter().enumerate() {
        let hi = bounds.get(i + 1).map_or(MAX, |next| next - 1);
        let segment = CharSet::range(lo, hi);
        if sets.iter().any(|s| !s.intersect(&segment).is_empty()) {
            segments.push(segment);
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ranges: &[(u32, u32)]) -> CharSet {
        CharSet::from_ranges(ranges)
    }

    #[test]
    fn ranges_merge() {
        assert_eq!(set(&[(5, 10), (1, 3), (4, 4)]).ranges(), [(1, 10)]);
        assert_eq!(set(&[(1, 2), (5, 6)]).ranges(), [(1, 2), (5, 6)]);
        assert_eq!(set(&[(10, 1)]).ranges(), [(1, 10)]);
    }

    #[test]
    fn complement_and_intersection() {
        let digits = set(&[(0x30, 0x39)]);
        let rest = digits.complement();
        assert!(!rest.contains(0x35));
        assert!(rest.contains(0) && rest.contains(MAX));
        assert!(digits.intersect(&rest).is_empty());
        assert_eq!(digits.union(&rest), CharSet::full());
        assert_eq!(CharSet::full().complement(), CharSet::empty());
    }

    #[test]
    fn representatives_prefer_letters() {
        assert_eq!(set(&[(0, 0x7A)]).representative(), Some('a'));
        assert_eq!(set(&[(0x30, 0x39)]).representative(), Some('0'));
        assert_eq!(set(&[(0xE9, 0xE9)]).representative(), Some('é'));
        assert_eq!(CharSet::empty().representative(), None);
    }

    #[test]
    fn partitions_separate_every_boundary() {
        let a = set(&[(0, 10)]);
        let b = set(&[(5, 20)]);
        let parts: Vec<Vec<(u32, u32)>> = partition(&[&a, &b])
            .iter()
            .map(|p| p.ranges().to_vec())
            .collect();
        assert_eq!(parts, [vec![(0, 4)], vec![(5, 10)], vec![(11, 20)]]);
    }
}
