//! Automata for parsed patterns.
//!
//! A pattern becomes a Thompson NFA whose edges read character sets or are
//! empty; `^` and `$` become assertion edges. Determinization tracks, for
//! each NFA state, the *phase* of the input: nothing read yet, reading, or
//! past a `$`. A `^` edge is only followed before anything is read, and
//! nothing may be read after a `$`. That handles anchors anywhere in a
//! pattern, not just at its ends.
//!
//! The DFA's alphabet is the coarsest partition of the code points on
//! which every edge set is uniform, so transitions are indexed by segment.

use std::collections::{HashMap, VecDeque};

use crate::charset::{CharSet, partition};
use crate::parse::{Error, Node};

/// NFA states beyond this make a pattern too large to analyse.
pub const MAX_NFA_STATES: usize = 20_000;
/// DFA states beyond this make a pattern too large to analyse.
pub const MAX_DFA_STATES: usize = 5_000;

#[derive(Clone, Debug)]
enum Edge {
    Empty(usize),
    Read(usize, usize),
    Start(usize),
    End(usize),
}

struct Nfa {
    edges: Vec<Vec<Edge>>,
    sets: Vec<CharSet>,
}

impl Nfa {
    fn state(&mut self) -> Result<usize, Error> {
        if self.edges.len() >= MAX_NFA_STATES {
            return Err(Error::Unsupported("the pattern is too large".into()));
        }
        self.edges.push(Vec::new());
        Ok(self.edges.len() - 1)
    }

    fn set(&mut self, set: &CharSet) -> usize {
        if let Some(i) = self.sets.iter().position(|s| s == set) {
            return i;
        }
        self.sets.push(set.clone());
        self.sets.len() - 1
    }

    /// Adds `node` between fresh states, returning (entry, exit).
    fn fragment(&mut self, node: &Node) -> Result<(usize, usize), Error> {
        let (entry, exit) = (self.state()?, self.state()?);
        self.connect(node, entry, exit)?;
        Ok((entry, exit))
    }

    /// Adds paths from `from` to `to` reading `node`.
    fn connect(&mut self, node: &Node, from: usize, to: usize) -> Result<(), Error> {
        match node {
            Node::Empty => self.edges[from].push(Edge::Empty(to)),
            Node::Set(set) => {
                let set = self.set(set);
                self.edges[from].push(Edge::Read(set, to));
            }
            Node::Start => self.edges[from].push(Edge::Start(to)),
            Node::End => self.edges[from].push(Edge::End(to)),
            Node::Concat(parts) => {
                let mut at = from;
                for (i, part) in parts.iter().enumerate() {
                    let next = if i + 1 == parts.len() {
                        to
                    } else {
                        self.state()?
                    };
                    self.connect(part, at, next)?;
                    at = next;
                }
            }
            Node::Alt(alternatives) => {
                for alternative in alternatives {
                    self.connect(alternative, from, to)?;
                }
            }
            Node::Repeat { node, min, max } => {
                let mut at = from;
                for _ in 0..*min {
                    let next = self.state()?;
                    self.connect(node, at, next)?;
                    at = next;
                }
                match max {
                    None => {
                        // A loop through a fresh fragment.
                        let (entry, exit) = self.fragment(node)?;
                        self.edges[at].push(Edge::Empty(entry));
                        self.edges[exit].push(Edge::Empty(entry));
                        self.edges[exit].push(Edge::Empty(to));
                        self.edges[at].push(Edge::Empty(to));
                    }
                    Some(max) => {
                        self.edges[at].push(Edge::Empty(to));
                        for _ in *min..*max {
                            let next = self.state()?;
                            self.connect(node, at, next)?;
                            self.edges[next].push(Edge::Empty(to));
                            at = next;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Input phases for anchor handling.
const BEFORE: u8 = 0;
const READING: u8 = 1;
const AFTER_END: u8 = 2;

/// Total NFA states across all DFA states beyond this make a pattern too
/// costly to analyse, however few DFA states there are.
pub const MAX_CONFIG_WORK: usize = 2_000_000;

/// A set of (NFA state, phase), sorted: the identity of a DFA state.
type Config = Vec<(usize, u8)>;

/// A deterministic automaton over segments of the code points.
#[derive(Clone, Debug)]
pub(crate) struct Dfa {
    /// The alphabet: disjoint sets, sorted by their first code point.
    pub(crate) segments: Vec<CharSet>,
    /// `next[state][segment]`; `None` is the dead state.
    pub(crate) next: Vec<Vec<Option<usize>>>,
    pub(crate) accepting: Vec<bool>,
}

impl Dfa {
    /// The automaton for `node`, matched against the whole input; with
    /// `search`, anywhere in the input, as JSON Schema's `pattern` is.
    pub(crate) fn build(node: &Node, search: bool) -> Result<Dfa, Error> {
        let mut nfa = Nfa {
            edges: Vec::new(),
            sets: Vec::new(),
        };
        let (start, accept) = if search {
            let any = nfa.set(&CharSet::full());
            let (start, accept) = (nfa.state()?, nfa.state()?);
            let (entry, exit) = nfa.fragment(node)?;
            nfa.edges[start].push(Edge::Read(any, start));
            nfa.edges[start].push(Edge::Empty(entry));
            nfa.edges[exit].push(Edge::Empty(accept));
            nfa.edges[accept].push(Edge::Read(any, accept));
            (start, accept)
        } else {
            nfa.fragment(node)?
        };
        determinize(&nfa, start, accept)
    }

    /// The segment containing `c`.
    pub(crate) fn segment_of(&self, c: u32) -> Option<usize> {
        self.segments
            .binary_search_by(|segment| {
                let (lo, hi) = (segment.ranges()[0].0, segment.ranges()[0].1);
                if hi < c {
                    std::cmp::Ordering::Less
                } else if lo > c {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
    }

    pub(crate) fn is_match(&self, text: &str) -> bool {
        let mut state = 0;
        for c in text.chars() {
            let Some(segment) = self.segment_of(c as u32) else {
                return false;
            };
            match self.next[state][segment] {
                Some(next) => state = next,
                None => return false,
            }
        }
        self.accepting[state]
    }
}

/// Membership stamps for building configurations without hashing.
struct Stamps {
    marks: Vec<u32>,
    epoch: u32,
}

impl Stamps {
    fn new(states: usize) -> Self {
        Self {
            marks: vec![0; states * 3],
            epoch: 0,
        }
    }

    fn next_epoch(&mut self) {
        self.epoch += 1;
    }

    /// Marks `(state, phase)`; `false` if it was already marked this epoch.
    fn mark(&mut self, state: usize, phase: u8) -> bool {
        let slot = &mut self.marks[state * 3 + usize::from(phase)];
        if *slot == self.epoch {
            false
        } else {
            *slot = self.epoch;
            true
        }
    }
}

/// Extends `config` (whose members are marked in `stamps`) along empty and
/// assertion edges, then sorts it.
fn closure(nfa: &Nfa, config: &mut Config, stamps: &mut Stamps) {
    let mut stack: Vec<(usize, u8)> = config.clone();
    while let Some((state, phase)) = stack.pop() {
        for edge in &nfa.edges[state] {
            let next = match *edge {
                Edge::Empty(to) => Some((to, phase)),
                Edge::Start(to) if phase == BEFORE => Some((to, BEFORE)),
                Edge::End(to) => Some((to, AFTER_END)),
                _ => None,
            };
            if let Some((to, phase)) = next
                && stamps.mark(to, phase)
            {
                config.push((to, phase));
                stack.push((to, phase));
            }
        }
    }
    config.sort_unstable();
}

fn determinize(nfa: &Nfa, start: usize, accept: usize) -> Result<Dfa, Error> {
    // Segments are whole ranges; split multi-range sets so each segment is
    // a single range, which `segment_of` relies on.
    let set_refs: Vec<&CharSet> = nfa.sets.iter().collect();
    let segments: Vec<CharSet> = partition(&set_refs)
        .into_iter()
        .flat_map(|s| {
            s.ranges()
                .iter()
                .map(|&(lo, hi)| CharSet::range(lo, hi))
                .collect::<Vec<_>>()
        })
        .collect();
    // For each NFA set, which segments it covers.
    let covers: Vec<Vec<bool>> = nfa
        .sets
        .iter()
        .map(|set| {
            segments
                .iter()
                .map(|segment| !set.intersect(segment).is_empty())
                .collect()
        })
        .collect();

    let mut stamps = Stamps::new(nfa.edges.len());
    stamps.next_epoch();
    stamps.mark(start, BEFORE);
    let mut initial: Config = vec![(start, BEFORE)];
    closure(nfa, &mut initial, &mut stamps);
    let mut work = initial.len();
    let mut ids: HashMap<Config, usize> = HashMap::new();
    let mut configs: Vec<Config> = Vec::new();
    let mut queue = VecDeque::new();
    ids.insert(initial.clone(), 0);
    configs.push(initial);
    queue.push_back(0usize);
    let mut next_table: Vec<Vec<Option<usize>>> = Vec::new();
    while let Some(id) = queue.pop_front() {
        let config = configs[id].clone();
        let mut row = Vec::with_capacity(segments.len());
        for (segment, _) in segments.iter().enumerate() {
            stamps.next_epoch();
            let mut target: Config = Vec::new();
            for &(state, phase) in &config {
                if phase == AFTER_END {
                    continue;
                }
                for edge in &nfa.edges[state] {
                    if let Edge::Read(set, to) = *edge
                        && covers[set][segment]
                        && stamps.mark(to, READING)
                    {
                        target.push((to, READING));
                    }
                }
            }
            if target.is_empty() {
                row.push(None);
                continue;
            }
            closure(nfa, &mut target, &mut stamps);
            let next = if let Some(&existing) = ids.get(&target) {
                existing
            } else {
                work += target.len();
                if configs.len() >= MAX_DFA_STATES || work > MAX_CONFIG_WORK {
                    return Err(Error::Unsupported(
                        "the pattern's automaton is too large".into(),
                    ));
                }
                let new = configs.len();
                ids.insert(target.clone(), new);
                configs.push(target);
                queue.push_back(new);
                new
            };
            row.push(Some(next));
        }
        if next_table.len() <= id {
            next_table.resize(id + 1, Vec::new());
        }
        next_table[id] = row;
    }
    next_table.resize(configs.len(), vec![None; segments.len()]);
    let accepting = configs
        .iter()
        .map(|c| c.iter().any(|&(state, _)| state == accept))
        .collect();
    Ok(Dfa {
        segments,
        next: next_table,
        accepting,
    })
}
