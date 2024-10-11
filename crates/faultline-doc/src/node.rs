use std::fmt;

use faultline_source::Span;
use indexmap::IndexMap;
use indexmap::map::Entry;

use crate::number::Number;
use crate::pointer::JsonPointer;

/// A value in a parsed document together with the span it came from.
///
/// JSON and YAML documents both lower to this tree. YAML features beyond the
/// JSON data model (anchors, merge keys, tags) are resolved during loading, so
/// consumers only ever see JSON-shaped data.
#[derive(Clone, PartialEq)]
pub struct Node {
    pub value: Value,
    pub span: Span,
}

#[derive(Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Node>),
    Object(Object),
}

/// The kind of a [`Value`], for messages such as "expected an object, found a
/// string".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl Kind {
    pub fn describe(self) -> &'static str {
        match self {
            Kind::Null => "null",
            Kind::Bool => "a boolean",
            Kind::Number => "a number",
            Kind::String => "a string",
            Kind::Array => "an array",
            Kind::Object => "an object",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

impl Node {
    pub fn new(value: Value, span: Span) -> Self {
        Self { value, span }
    }

    pub fn kind(&self) -> Kind {
        match &self.value {
            Value::Null => Kind::Null,
            Value::Bool(_) => Kind::Bool,
            Value::Number(_) => Kind::Number,
            Value::String(_) => Kind::String,
            Value::Array(_) => Kind::Array,
            Value::Object(_) => Kind::Object,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.value {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match &self.value {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Node]> {
        match &self.value {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match &self.value {
            Value::Object(obj) => Some(obj),
            _ => None,
        }
    }

    /// Looks up `key` if this node is an object.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_object()?.get(key)
    }

    /// Resolves a JSON Pointer relative to this node.
    pub fn pointer(&self, pointer: &JsonPointer) -> Option<&Node> {
        pointer
            .tokens()
            .iter()
            .try_fold(self, |node, token| match &node.value {
                Value::Object(obj) => obj.get(token),
                Value::Array(items) => array_index(token).and_then(|i| items.get(i)),
                _ => None,
            })
    }

    /// Deep equality of the data, ignoring spans and object member order.
    ///
    /// This is the equality JSON Schema uses for `enum` and `const`, except
    /// that numbers compare exactly rather than as floats.
    pub fn same_value(&self, other: &Node) -> bool {
        match (&self.value, &other.value) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same_value(y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.get(key).is_some_and(|other| value.same_value(other)))
            }
            _ => false,
        }
    }

    /// Total number of nodes in this subtree, including `self`.
    pub fn count_nodes(&self) -> u64 {
        1 + match &self.value {
            Value::Array(items) => items.iter().map(Node::count_nodes).sum(),
            Value::Object(obj) => obj.values().map(Node::count_nodes).sum(),
            _ => 0,
        }
    }
}

/// Parses an RFC 6901 array index: `0` or a decimal without leading zeros.
fn array_index(token: &str) -> Option<usize> {
    let valid = token == "0"
        || (!token.is_empty()
            && !token.starts_with('0')
            && token.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| token.parse().ok()).flatten()
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.value, f)
    }
}

impl fmt::Debug for Value {
    /// Formats as compact JSON-like data; spans are omitted to keep test
    /// failures readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Number(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::Array(items) => f.debug_list().entries(items).finish(),
            Value::Object(obj) => fmt::Debug::fmt(obj, f),
        }
    }
}

/// An object member: the key's span and the value.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub key_span: Span,
    pub value: Node,
}

/// An object whose members keep their document order.
#[derive(Clone, Default, PartialEq)]
pub struct Object {
    members: IndexMap<String, Member>,
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a member. If the key already exists, the new value replaces the
    /// old one in place (keeping the original position) and the old member is
    /// returned so the caller can report the duplicate.
    pub fn insert(&mut self, key: String, key_span: Span, value: Node) -> Option<Member> {
        match self.members.entry(key) {
            Entry::Occupied(mut slot) => Some(std::mem::replace(
                slot.get_mut(),
                Member { key_span, value },
            )),
            Entry::Vacant(slot) => {
                slot.insert(Member { key_span, value });
                None
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.members.get(key).map(|m| &m.value)
    }

    pub fn member(&self, key: &str) -> Option<&Member> {
        self.members.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.members.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Members in document order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&str, &Node)> {
        self.members.iter().map(|(k, m)| (k.as_str(), &m.value))
    }

    /// Members in document order, with mutable values, for front ends that
    /// rewrite trees (expanding RAML includes).
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = (&str, &mut Node)> {
        self.members
            .iter_mut()
            .map(|(k, m)| (k.as_str(), &mut m.value))
    }

    pub fn members(&self) -> impl ExactSizeIterator<Item = (&str, &Member)> {
        self.members.iter().map(|(k, m)| (k.as_str(), m))
    }

    pub fn keys(&self) -> impl ExactSizeIterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    pub fn values(&self) -> impl ExactSizeIterator<Item = &Node> {
        self.members.values().map(|m| &m.value)
    }
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::number::Decimal;
    use faultline_source::SourceId;

    fn span(start: u32) -> Span {
        Span::new(SourceId::from_raw(0), start, start + 1)
    }

    fn num(s: &str) -> Node {
        Node::new(Value::Number(Decimal::parse(s).unwrap().into()), span(0))
    }

    fn obj(entries: &[(&str, Node)]) -> Node {
        let mut o = Object::new();
        for (i, (k, v)) in entries.iter().enumerate() {
            o.insert((*k).to_owned(), span(i as u32), v.clone());
        }
        Node::new(Value::Object(o), span(0))
    }

    #[test]
    fn duplicate_insert_keeps_position_and_returns_previous() {
        let mut o = Object::new();
        assert!(o.insert("a".into(), span(0), num("1")).is_none());
        assert!(o.insert("b".into(), span(1), num("2")).is_none());
        let old = o.insert("a".into(), span(2), num("3")).unwrap();
        assert_eq!(old.key_span, span(0));
        assert_eq!(o.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert!(o.get("a").unwrap().same_value(&num("3")));
    }

    #[test]
    fn same_value_ignores_order_spans_and_number_spelling() {
        let a = obj(&[("x", num("1.0")), ("y", num("2"))]);
        let b = obj(&[("y", num("2e0")), ("x", num("1"))]);
        assert!(a.same_value(&b));
        let c = obj(&[("x", num("1"))]);
        assert!(!a.same_value(&c));
    }

    #[test]
    fn pointer_resolution_follows_rfc6901_array_rules() {
        let arr = Node::new(Value::Array(vec![num("10"), num("11")]), span(0));
        let root = obj(&[("a/b", arr)]);
        let get = |p: &str| root.pointer(&JsonPointer::parse(p).unwrap()).cloned();
        assert!(get("/a~1b/1").unwrap().same_value(&num("11")));
        assert!(get("/a~1b/01").is_none());
        assert!(get("/a~1b/-").is_none());
        assert!(get("/a~1b/2").is_none());
        assert!(get("").unwrap().same_value(&root));
    }

    #[test]
    fn node_count_includes_all_descendants() {
        let root = obj(&[
            ("a", num("1")),
            ("b", Node::new(Value::Array(vec![num("1")]), span(0))),
        ]);
        assert_eq!(root.count_nodes(), 4);
    }
}
