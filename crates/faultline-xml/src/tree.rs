//! The element tree an XML document is read into.

use faultline_source::Span;

/// A name with its namespace resolved: `{http://www.w3.org/2001/XMLSchema}element`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QName {
    /// The namespace URI, or `None` for names in no namespace.
    pub namespace: Option<String>,
    pub local: String,
}

impl QName {
    pub fn new(namespace: Option<&str>, local: impl Into<String>) -> Self {
        Self {
            namespace: namespace.map(str::to_owned),
            local: local.into(),
        }
    }

    /// Whether this is `local` in `namespace`.
    pub fn is(&self, namespace: &str, local: &str) -> bool {
        self.local == local && self.namespace.as_deref() == Some(namespace)
    }
}

impl std::fmt::Display for QName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.namespace {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}

/// An attribute of an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: QName,
    /// The attribute's value, with references expanded and whitespace
    /// normalized as XML requires.
    pub value: String,
    /// The whole `name="value"`.
    pub span: Span,
    /// The value between its quotes.
    pub value_span: Span,
}

/// An element and everything inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub name: QName,
    /// The name as written, prefix included (`xs:element`), for messages.
    pub raw_name: String,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Content>,
    /// The namespace prefixes in scope at this element, innermost first:
    /// needed to resolve QName-valued attributes (`type="tns:Pet"`).
    pub namespaces: Vec<(String, String)>,
    /// From `<` of the start tag to `>` of the end tag.
    pub span: Span,
    /// The start tag's name.
    pub name_span: Span,
}

/// Content inside an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Element(Element),
    /// Character data, with references and CDATA sections expanded.
    Text(String, Span),
}

impl Element {
    /// The value of the unqualified attribute `local`.
    pub fn attribute(&self, local: &str) -> Option<&Attribute> {
        self.attributes
            .iter()
            .find(|a| a.name.namespace.is_none() && a.name.local == local)
    }

    /// The value of the unqualified attribute `local`, as a string.
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attribute(local).map(|a| a.value.as_str())
    }

    /// Child elements, in document order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Content::Element(e) => Some(e),
            Content::Text(..) => None,
        })
    }

    /// Child elements named `local` in `namespace`.
    pub fn children_named<'a>(
        &'a self,
        namespace: &'a str,
        local: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.elements().filter(move |e| e.name.is(namespace, local))
    }

    /// The first child element named `local` in `namespace`.
    pub fn child(&self, namespace: &str, local: &str) -> Option<&Element> {
        self.elements().find(|e| e.name.is(namespace, local))
    }

    /// All character data directly inside this element, concatenated.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|c| match c {
                Content::Text(text, _) => Some(text.as_str()),
                Content::Element(_) => None,
            })
            .collect()
    }

    /// The namespace URI bound to `prefix` here (`""` for the default
    /// namespace).
    pub fn namespace_of(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(crate::XML_NAMESPACE);
        }
        self.namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, uri)| uri.as_str())
            .filter(|uri| !uri.is_empty())
    }

    /// Resolves a `QName` written in an attribute value (`tns:Pet`, or
    /// `Pet` in the default namespace), as XML Schema and WSDL do.
    /// `None` when the prefix is not bound.
    pub fn resolve_qname(&self, text: &str) -> Option<QName> {
        let text = text.trim();
        match text.split_once(':') {
            Some((prefix, local)) => Some(QName::new(Some(self.namespace_of(prefix)?), local)),
            None => Some(QName::new(self.namespace_of(""), text)),
        }
    }
}

/// A parsed document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub root: Element,
}
