//! ECMA-262 regular expressions as finite automata.
//!
//! JSON Schema's `pattern` is an ECMA-262 regular expression that a string
//! must contain a match of. Comparing two versions of a schema asks whether
//! every string one pattern accepts is accepted by the other. For patterns
//! that describe regular languages, which is nearly all of them in API
//! descriptions, that question is decidable. This crate answers it with
//! automata:
//!
//! * [`Regex::new`] parses a pattern and builds a DFA over code points,
//!   with JSON Schema's search semantics (`abc` accepts `xabcx`; `^` and `$`
//!   anchor);
//! * [`Regex::is_match`] decides membership exactly;
//! * [`difference`] finds the shortest string (within length bounds)
//!   accepted by one pattern and not another, or proves there is none;
//! * [`Regex::samples`] produces short members, for generating test values.
//!
//! Patterns using backreferences, lookaround, word boundaries or Unicode
//! property escapes are rejected as [`Error::Unsupported`], as are patterns
//! whose automata exceed fixed size limits, so callers can fall back to
//! "unknown" rather than guess.

mod automaton;
pub mod charset;
mod parse;

use std::collections::{HashMap, VecDeque};

pub use automaton::{MAX_CONFIG_WORK, MAX_DFA_STATES, MAX_NFA_STATES};
pub use parse::{Error, MAX_REPEAT, Node, parse};

use automaton::Dfa;
use charset::{CharSet, partition};

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    pattern: String,
    dfa: Dfa,
}

impl Regex {
    /// Compiles `pattern` with JSON Schema semantics: a string is accepted
    /// if it contains a match.
    pub fn new(pattern: &str) -> Result<Regex, Error> {
        let node = parse::parse(pattern)?;
        Ok(Regex {
            pattern: pattern.to_owned(),
            dfa: Dfa::build(&node, true)?,
        })
    }

    /// The language of every string.
    pub fn any() -> Regex {
        Regex::new("").expect("the empty pattern compiles")
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.dfa.is_match(text)
    }

    /// Up to `count` accepted strings, shortest first, each within
    /// `min_len..=max_len` characters.
    pub fn samples(&self, count: usize, min_len: usize, max_len: Option<usize>) -> Vec<String> {
        let everything = Regex::never();
        let mut out = Vec::new();
        search(&self.dfa, &everything.dfa, min_len, max_len, |text| {
            out.push(text);
            out.len() >= count
        });
        out
    }

    /// The empty language, as the right side of a difference.
    fn never() -> Regex {
        Regex {
            pattern: String::from("(?!)"),
            dfa: Dfa {
                segments: vec![CharSet::full()],
                next: vec![vec![None]],
                accepting: vec![false],
            },
        }
    }
}

/// The shortest string accepted by `left` and not by `right`, with a length
/// in `min_len..=max_len` characters; `None` if there is none, which proves
/// that every such string of `left` is accepted by `right`.
pub fn difference(
    left: &Regex,
    right: &Regex,
    min_len: usize,
    max_len: Option<usize>,
) -> Option<String> {
    let mut found = None;
    search(&left.dfa, &right.dfa, min_len, max_len, |text| {
        found = Some(text);
        true
    });
    found
}

/// Largest length bound applied during searches. A larger `min_len` is
/// treated as this, and a larger `max_len` is not applied, so a string
/// found may violate such bounds; callers validate witnesses against the
/// whole type. A proof that no string exists is unaffected.
const MAX_TRACKED_LENGTH: usize = 256;

/// A search state: (left state, right state or dead, length so far capped).
type Key = (usize, Option<usize>, usize);

/// Breadth-first search over pairs of states, reporting strings accepted by
/// `left` and rejected by `right` in order of length until `report` returns
/// `true`.
fn search(
    left: &Dfa,
    right: &Dfa,
    min_len: usize,
    max_len: Option<usize>,
    mut report: impl FnMut(String) -> bool,
) {
    if max_len.is_some_and(|max| max < min_len) {
        return;
    }
    let min_len = min_len.min(MAX_TRACKED_LENGTH);
    let max_len = max_len.filter(|&max| max <= MAX_TRACKED_LENGTH);
    let cap = max_len.map_or(min_len, |max| max.max(min_len));
    // A shared alphabet: each combined segment lies within one segment of
    // each automaton, or outside it (the dead state).
    let left_refs: Vec<&CharSet> = left.segments.iter().collect();
    let right_refs: Vec<&CharSet> = right.segments.iter().collect();
    let all: Vec<&CharSet> = left_refs.iter().chain(&right_refs).copied().collect();
    let alphabet: Vec<(CharSet, char, Option<usize>, Option<usize>)> = partition(&all)
        .into_iter()
        .filter_map(|segment| {
            let first = segment.ranges().first()?.0;
            let left_segment = left.segment_of(first)?;
            let right_segment = right.segment_of(first);
            let representative = segment.representative()?;
            Some((segment, representative, Some(left_segment), right_segment))
        })
        .collect();

    let mut parent: HashMap<Key, Option<(Key, char)>> = HashMap::new();
    let start: Key = (0, Some(0), 0);
    parent.insert(start, None);
    let mut queue = VecDeque::from([start]);
    while let Some(key @ (l, r, len)) = queue.pop_front() {
        let right_accepts = r.is_some_and(|r| right.accepting[r]);
        if left.accepting[l]
            && !right_accepts
            && len >= min_len
            && report(reconstruct(&parent, key))
        {
            return;
        }
        if max_len.is_some_and(|max| len >= max) {
            continue;
        }
        let next_len = if len < cap { len + 1 } else { len };
        for &(_, c, left_segment, right_segment) in &alphabet {
            let Some(next_l) = left_segment.and_then(|s| left.next[l][s]) else {
                continue;
            };
            let next_r = match (r, right_segment) {
                (Some(r), Some(s)) => right.next[r][s],
                _ => None,
            };
            let next = (next_l, next_r, next_len);
            if let std::collections::hash_map::Entry::Vacant(entry) = parent.entry(next) {
                entry.insert(Some((key, c)));
                queue.push_back(next);
            }
        }
    }
}

fn reconstruct<K: Copy + Eq + std::hash::Hash>(
    parent: &HashMap<K, Option<(K, char)>>,
    mut key: K,
) -> String {
    let mut chars = Vec::new();
    while let Some(Some((previous, c))) = parent.get(&key) {
        chars.push(*c);
        key = *previous;
    }
    chars.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(pattern: &str) -> Regex {
        Regex::new(pattern).unwrap()
    }

    #[test]
    fn search_semantics_and_anchors() {
        assert!(re("abc").is_match("xxabcxx"));
        assert!(!re("^abc").is_match("xabc"));
        assert!(re("^abc").is_match("abcx"));
        assert!(!re("abc$").is_match("abcx"));
        assert!(re("^abc$").is_match("abc"));
        assert!(re("^a|b$").is_match("axx"));
        assert!(re("^a|b$").is_match("xxb"));
        assert!(!re("^a|b$").is_match("xbx"));
        assert!(re("").is_match(""));
        assert!(!re("a$b").is_match("ab"), "nothing can follow the end");
    }

    #[test]
    fn character_classes_and_repetition() {
        let id = re("^[A-Z]{2}-\\d{3,4}$");
        assert!(id.is_match("AB-123"));
        assert!(id.is_match("AB-1234"));
        assert!(!id.is_match("AB-12"));
        assert!(!id.is_match("ab-123"));
        let email = re(r"^[^@\s]+@[^@\s]+\.[a-z]{2,}$");
        assert!(email.is_match("a@b.io"));
        assert!(!email.is_match("a@b"));
        assert!(re("^.$").is_match("é"));
        assert!(!re("^.$").is_match("\n"));
    }

    #[test]
    fn differences_are_shortest_and_respect_lengths() {
        assert_eq!(
            difference(&re("^[a-z]+$"), &re("^[a-y]+$"), 0, None).as_deref(),
            Some("z")
        );
        assert_eq!(
            difference(&re("^[a-z]{2}$"), &re("^[a-z]+$"), 0, None),
            None
        );
        assert_eq!(
            difference(&re("^[0-9]+$"), &re("^[0-9]{1,3}$"), 0, None).as_deref(),
            Some("0000")
        );
        // Within lengths: shorter differences are skipped.
        let witness = difference(&Regex::any(), &re("^a"), 3, Some(5)).unwrap();
        assert!(
            witness.chars().count() >= 3 && !witness.starts_with('a'),
            "{witness}"
        );
        assert_eq!(
            difference(&re("^a{10}$"), &Regex::never(), 0, Some(5)),
            None
        );
    }

    #[test]
    fn any_includes_everything() {
        assert_eq!(difference(&re("^[a-z]+$"), &Regex::any(), 0, None), None);
        assert!(difference(&Regex::any(), &re("x"), 0, None).is_some());
    }

    #[test]
    fn samples_are_members() {
        let r = re("^(cat|dog)s?$");
        let samples = r.samples(4, 0, None);
        assert_eq!(samples[0], "cat");
        assert!(samples.iter().all(|s| r.is_match(s)));
        assert!(re("^a{3}$").samples(2, 5, None).is_empty());
    }
}
