//! Evaluating a query against a document tree.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use faultline_doc::{Node, Number, Object, Value};
use faultline_regex::Regex;

use crate::ast::{
    Argument, Comparable, CompareOp, Function, FunctionName, Literal, Logical, Query, Segment,
    Selector,
};

/// One step from a node to a child.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step {
    Name(String),
    Index(usize),
}

/// Where a node is: the steps from the root to it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Location(pub Vec<Step>);

impl Location {
    fn child(&self, step: Step) -> Location {
        let mut steps = self.0.clone();
        steps.push(step);
        Location(steps)
    }
}

/// The normalized path RFC 9535 defines: `$['store']['book'][0]`.
impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("$")?;
        for step in &self.0 {
            match step {
                Step::Index(i) => write!(f, "[{i}]")?,
                Step::Name(name) => {
                    f.write_str("['")?;
                    for c in name.chars() {
                        match c {
                            '\'' => f.write_str("\\'")?,
                            '\\' => f.write_str("\\\\")?,
                            '\u{8}' => f.write_str("\\b")?,
                            '\u{c}' => f.write_str("\\f")?,
                            '\n' => f.write_str("\\n")?,
                            '\r' => f.write_str("\\r")?,
                            '\t' => f.write_str("\\t")?,
                            '\u{0}'..='\u{1f}' => write!(f, "\\u{:04x}", c as u32)?,
                            c => write!(f, "{c}")?,
                        }
                    }
                    f.write_str("']")?;
                }
            }
        }
        Ok(())
    }
}

/// A node a query selected, and where it is.
#[derive(Clone, Debug)]
pub struct Match<'n> {
    pub location: Location,
    pub node: &'n Node,
}

/// The nodes a query visits at most, by default: generous for descriptions
/// of any real size, and a bound for queries whose filters query the
/// document again for every node (`$..*[?@..*]`).
pub const DEFAULT_BUDGET: u64 = 20_000_000;

/// A query visited more nodes than its budget allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub budget: u64,
}

impl fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the query visits more than {} nodes", self.budget)
    }
}

impl std::error::Error for BudgetExceeded {}

impl Query {
    /// The nodes the query selects from `root`, in the order RFC 9535
    /// gives: document order for wildcards and descendants. A query that
    /// exceeds [`DEFAULT_BUDGET`] selects nothing; use
    /// [`try_select`](Self::try_select) to tell.
    pub fn select<'n>(&self, root: &'n Node) -> Vec<Match<'n>> {
        self.try_select(root, DEFAULT_BUDGET).unwrap_or_default()
    }

    /// The nodes the query selects, visiting at most `budget` nodes.
    pub fn try_select<'n>(
        &self,
        root: &'n Node,
        budget: u64,
    ) -> Result<Vec<Match<'n>>, BudgetExceeded> {
        let mut evaluator = Evaluator {
            root,
            regexes: HashMap::new(),
            steps: 0,
            budget,
        };
        let selected = evaluator.query(self, root, &Location::default());
        if evaluator.steps > budget {
            return Err(BudgetExceeded { budget });
        }
        Ok(selected)
    }
}

struct Evaluator<'n> {
    root: &'n Node,
    /// Compiled patterns of `match` and `search`, `None` when invalid.
    regexes: HashMap<(String, bool), Option<Regex>>,
    /// Nodes visited so far; evaluation stops once past `budget`.
    steps: u64,
    budget: u64,
}

impl<'n> Evaluator<'n> {
    fn exhausted(&self) -> bool {
        self.steps > self.budget
    }

    /// Counts `count` visited nodes.
    fn visit(&mut self, count: usize) {
        self.steps = self.steps.saturating_add(count as u64);
    }

    fn query(&mut self, query: &Query, current: &'n Node, at: &Location) -> Vec<Match<'n>> {
        self.visit(1);
        if self.exhausted() {
            return Vec::new();
        }
        let (start, location) = if query.relative {
            (current, at.clone())
        } else {
            (self.root, Location::default())
        };
        let mut nodes = vec![Match {
            location,
            node: start,
        }];
        for segment in &query.segments {
            nodes = self.segment(segment, &nodes);
        }
        nodes
    }

    fn segment(&mut self, segment: &Segment, input: &[Match<'n>]) -> Vec<Match<'n>> {
        let mut out = Vec::new();
        for m in input {
            if self.exhausted() {
                return Vec::new();
            }
            if segment.descendant {
                let all = descendants(m);
                self.visit(all.len());
                for d in all {
                    if self.exhausted() {
                        return Vec::new();
                    }
                    self.selectors(&segment.selectors, &d, &mut out);
                }
            } else {
                self.selectors(&segment.selectors, m, &mut out);
            }
        }
        out
    }

    fn selectors(&mut self, selectors: &[Selector], m: &Match<'n>, out: &mut Vec<Match<'n>>) {
        for selector in selectors {
            match (selector, &m.node.value) {
                (Selector::Name(name), Value::Object(object)) => {
                    if let Some(child) = object.get(name) {
                        out.push(Match {
                            location: m.location.child(Step::Name(name.clone())),
                            node: child,
                        });
                    }
                }
                (Selector::Wildcard, _) => {
                    let kids = children(m);
                    self.visit(kids.len());
                    out.extend(kids);
                }
                (Selector::Index(i), Value::Array(items)) => {
                    if let Some(index) = normalize(*i, items.len()) {
                        out.push(Match {
                            location: m.location.child(Step::Index(index)),
                            node: &items[index],
                        });
                    }
                }
                (Selector::Slice { start, end, step }, Value::Array(items)) => {
                    for index in slice(*start, *end, *step, items.len()) {
                        out.push(Match {
                            location: m.location.child(Step::Index(index)),
                            node: &items[index],
                        });
                    }
                }
                (Selector::Filter(expr), _) => {
                    let kids = children(m);
                    self.visit(kids.len());
                    for child in kids {
                        if self.exhausted() {
                            return;
                        }
                        if self.logical(expr, child.node, &child.location) {
                            out.push(child);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn logical(&mut self, expr: &Logical, current: &'n Node, at: &Location) -> bool {
        match expr {
            Logical::Or(terms) => terms.iter().any(|t| self.logical(t, current, at)),
            Logical::And(terms) => terms.iter().all(|t| self.logical(t, current, at)),
            Logical::Not(inner) => !self.logical(inner, current, at),
            Logical::Exists(query) => !self.query(query, current, at).is_empty(),
            Logical::Function(f) => matches!(self.function(f, current, at), Val::Bool(true)),
            Logical::Comparison(left, op, right) => {
                let left = self.comparable(left, current, at);
                let right = self.comparable(right, current, at);
                compare(&left, *op, &right)
            }
        }
    }

    fn comparable(&mut self, c: &Comparable, current: &'n Node, at: &Location) -> Val<'n> {
        match c {
            Comparable::Literal(l) => literal(l),
            Comparable::Query(q) => single(&self.query(q, current, at)),
            Comparable::Function(f) => self.function(f, current, at),
        }
    }

    fn value_argument(&mut self, a: &Argument, current: &'n Node, at: &Location) -> Val<'n> {
        match a {
            Argument::Literal(l) => literal(l),
            Argument::Query(q) => single(&self.query(q, current, at)),
            Argument::Function(f) => self.function(f, current, at),
            Argument::Logical(l) => Val::Bool(self.logical(l, current, at)),
        }
    }

    fn nodes_argument(&mut self, a: &Argument, current: &'n Node, at: &Location) -> Vec<Match<'n>> {
        match a {
            Argument::Query(q) => self.query(q, current, at),
            _ => Vec::new(),
        }
    }

    fn function(&mut self, f: &Function, current: &'n Node, at: &Location) -> Val<'n> {
        match f.name {
            FunctionName::Length => match self.value_argument(&f.arguments[0], current, at) {
                Val::String(s) => count(s.chars().count()),
                Val::Array(items) => count(items.len()),
                Val::Object(object) => count(object.len()),
                _ => Val::Nothing,
            },
            FunctionName::Count => count(self.nodes_argument(&f.arguments[0], current, at).len()),
            FunctionName::Value => single(&self.nodes_argument(&f.arguments[0], current, at)),
            FunctionName::Match | FunctionName::Search => {
                let text = self.value_argument(&f.arguments[0], current, at);
                let pattern = self.value_argument(&f.arguments[1], current, at);
                let (Val::String(text), Val::String(pattern)) = (text, pattern) else {
                    return Val::Bool(false);
                };
                let whole = f.name == FunctionName::Match;
                let regex = self
                    .regexes
                    .entry((pattern.to_string(), whole))
                    .or_insert_with(|| compile(&pattern, whole));
                Val::Bool(regex.as_ref().is_some_and(|r| r.is_match(&text)))
            }
        }
    }
}

/// A value in a filter: a node's, a literal's, a function's, or nothing
/// (a query that selected no node, a function without a result).
#[derive(Clone, Debug)]
enum Val<'n> {
    Nothing,
    Null,
    Bool(bool),
    Number(Number),
    String(Cow<'n, str>),
    Array(&'n [Node]),
    Object(&'n Object),
}

fn node_value(node: &Node) -> Val<'_> {
    match &node.value {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => Val::Number(n.clone()),
        Value::String(s) => Val::String(Cow::Borrowed(s)),
        Value::Array(items) => Val::Array(items),
        Value::Object(object) => Val::Object(object),
    }
}

fn literal(l: &Literal) -> Val<'static> {
    match l {
        Literal::Null => Val::Null,
        Literal::Bool(b) => Val::Bool(*b),
        Literal::Number(d) => Val::Number(Number::Finite(d.clone())),
        Literal::String(s) => Val::String(Cow::Owned(s.clone())),
    }
}

fn count(n: usize) -> Val<'static> {
    Val::Number(Number::Finite(faultline_doc::Decimal::from_u128(n as u128)))
}

/// The value of a node list that holds exactly one node.
fn single<'n>(nodes: &[Match<'n>]) -> Val<'n> {
    match nodes {
        [one] => node_value(one.node),
        _ => Val::Nothing,
    }
}

fn compare(left: &Val<'_>, op: CompareOp, right: &Val<'_>) -> bool {
    match op {
        CompareOp::Eq => equal(left, right),
        CompareOp::Ne => !equal(left, right),
        CompareOp::Lt => less(left, right),
        CompareOp::Gt => less(right, left),
        CompareOp::Le => less(left, right) || equal(left, right),
        CompareOp::Ge => less(right, left) || equal(left, right),
    }
}

fn equal(a: &Val<'_>, b: &Val<'_>) -> bool {
    match (a, b) {
        (Val::Nothing, Val::Nothing) | (Val::Null, Val::Null) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Number(x), Val::Number(y)) => number_order(x, y) == Some(Ordering::Equal),
        (Val::String(x), Val::String(y)) => x == y,
        (Val::Array(x), Val::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| p.same_value(q))
        }
        (Val::Object(x), Val::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| v.same_value(w)))
        }
        _ => false,
    }
}

/// `<`, defined for numbers and for strings (by code point).
fn less(a: &Val<'_>, b: &Val<'_>) -> bool {
    match (a, b) {
        (Val::Number(x), Val::Number(y)) => number_order(x, y) == Some(Ordering::Less),
        (Val::String(x), Val::String(y)) => x < y,
        _ => false,
    }
}

/// Numbers in order, infinities at the ends; `NaN` is not ordered.
fn number_order(a: &Number, b: &Number) -> Option<Ordering> {
    let rank = |n: &Number| match n {
        Number::NegInfinity => Some(0),
        Number::Finite(_) => Some(1),
        Number::PosInfinity => Some(2),
        Number::NaN => None,
    };
    match (a, b) {
        (Number::Finite(x), Number::Finite(y)) => Some(x.cmp(y)),
        _ => Some(rank(a)?.cmp(&rank(b)?)),
    }
}

/// The index an index selector names, counting from the end if negative.
fn normalize(i: i64, len: usize) -> Option<usize> {
    let len = i64::try_from(len).ok()?;
    let index = if i < 0 { len + i } else { i };
    (0..len)
        .contains(&index)
        .then(|| usize::try_from(index).expect("in range"))
}

/// The indices a slice selects, as RFC 9535 section 2.3.4.2 computes them.
fn slice(start: Option<i64>, end: Option<i64>, step: Option<i64>, len: usize) -> Vec<usize> {
    let step = step.unwrap_or(1);
    let Ok(n) = i64::try_from(len) else {
        return Vec::new();
    };
    if step == 0 || n == 0 {
        return Vec::new();
    }
    let norm = |i: i64| if i >= 0 { i } else { n + i };
    let mut out = Vec::new();
    let to_index = |i: i64| usize::try_from(i).expect("within bounds");
    if step > 0 {
        let lower = norm(start.unwrap_or(0)).clamp(0, n);
        let upper = norm(end.unwrap_or(n)).clamp(0, n);
        let mut i = lower;
        while i < upper {
            out.push(to_index(i));
            i += step;
        }
    } else {
        let upper = norm(start.unwrap_or(n - 1)).clamp(-1, n - 1);
        let lower = norm(end.unwrap_or(-n - 1)).clamp(-1, n - 1);
        let mut i = upper;
        while lower < i {
            out.push(to_index(i));
            i += step;
        }
    }
    out
}

/// A node's members or items, in document order.
fn children<'n>(m: &Match<'n>) -> Vec<Match<'n>> {
    match &m.node.value {
        Value::Object(object) => object
            .iter()
            .map(|(k, v)| Match {
                location: m.location.child(Step::Name(k.to_owned())),
                node: v,
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| Match {
                location: m.location.child(Step::Index(i)),
                node: v,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A node and all its descendants, each before its own descendants.
fn descendants<'n>(m: &Match<'n>) -> Vec<Match<'n>> {
    let mut out = Vec::new();
    let mut stack = vec![m.clone()];
    while let Some(next) = stack.pop() {
        let mut kids = children(&next);
        out.push(next);
        kids.reverse();
        stack.extend(kids);
    }
    out
}

/// Compiles an I-Regexp (RFC 9485) pattern, for the whole string or for a
/// substring. `None` if the pattern is not one the regex engine accepts,
/// which makes the function false, as RFC 9535 requires for invalid
/// patterns.
fn compile(pattern: &str, whole: bool) -> Option<Regex> {
    let translated = i_regexp(pattern);
    let source = if whole {
        format!("^(?:{translated})$")
    } else {
        translated
    };
    Regex::new(&source).ok()
}

/// Rewrites I-Regexp into the ECMA-262 syntax the regex engine reads:
/// outside character classes `.` excludes only line feeds and carriage
/// returns, and `^` and `$` are ordinary characters.
fn i_regexp(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            '.' if !in_class => out.push_str("[^\\n\\r]"),
            '^' | '$' if !in_class => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}
