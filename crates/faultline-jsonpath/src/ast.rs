//! The syntax of a JSONPath query.

use faultline_doc::Decimal;

/// A query: `$` or `@` followed by segments.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// `@` (relative to the current node in a filter) rather than `$`.
    pub relative: bool,
    pub segments: Vec<Segment>,
}

impl Query {
    /// Whether the query selects at most one node: every segment is a
    /// child segment with a single name or index selector.
    pub fn is_singular(&self) -> bool {
        self.segments.iter().all(|s| {
            !s.descendant
                && matches!(
                    s.selectors.as_slice(),
                    [Selector::Name(_) | Selector::Index(_)]
                )
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// `..`: the selectors apply to the node and all its descendants.
    pub descendant: bool,
    pub selectors: Vec<Selector>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Selector {
    Name(String),
    Wildcard,
    Index(i64),
    Slice {
        start: Option<i64>,
        end: Option<i64>,
        step: Option<i64>,
    },
    Filter(Logical),
}

/// A filter's logical expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Logical {
    Or(Vec<Logical>),
    And(Vec<Logical>),
    Not(Box<Logical>),
    Comparison(Box<Comparable>, CompareOp, Box<Comparable>),
    /// A query's result is non-empty.
    Exists(Query),
    /// A function returning a logical value (`match`, `search`).
    Function(Function),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A side of a comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Comparable {
    Literal(Literal),
    /// A query that selects at most one node.
    Query(Query),
    /// A function returning a value (`length`, `count`, `value`).
    Function(Function),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Null,
    Bool(bool),
    Number(Decimal),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: FunctionName,
    pub arguments: Vec<Argument>,
}

/// The functions RFC 9535 defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionName {
    Length,
    Count,
    Match,
    Search,
    Value,
}

impl FunctionName {
    pub fn parse(name: &str) -> Option<FunctionName> {
        Some(match name {
            "length" => FunctionName::Length,
            "count" => FunctionName::Count,
            "match" => FunctionName::Match,
            "search" => FunctionName::Search,
            "value" => FunctionName::Value,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FunctionName::Length => "length",
            FunctionName::Count => "count",
            FunctionName::Match => "match",
            FunctionName::Search => "search",
            FunctionName::Value => "value",
        }
    }

    /// The types of the parameters.
    pub fn parameters(self) -> &'static [Type] {
        match self {
            FunctionName::Length => &[Type::Value],
            FunctionName::Count | FunctionName::Value => &[Type::Nodes],
            FunctionName::Match | FunctionName::Search => &[Type::Value, Type::Value],
        }
    }

    pub fn result(self) -> Type {
        match self {
            FunctionName::Length | FunctionName::Count | FunctionName::Value => Type::Value,
            FunctionName::Match | FunctionName::Search => Type::Logical,
        }
    }
}

/// The types of RFC 9535's function extensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Value,
    Logical,
    Nodes,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Argument {
    Literal(Literal),
    Query(Query),
    Logical(Logical),
    Function(Function),
}
