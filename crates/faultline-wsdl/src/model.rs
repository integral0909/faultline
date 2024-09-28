//! The components of a WSDL 1.1 description, as read from it.

use faultline_source::Span;
use faultline_xml::QName;

/// A message: the parts a request, response or fault carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub name: QName,
    pub parts: Vec<Part>,
    pub span: Span,
}

/// A part of a message: a global element (document style) or a value of a
/// type (RPC style).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub element: Option<(QName, Span)>,
    pub ty: Option<(QName, Span)>,
    pub span: Span,
}

/// A reference to a message from an operation.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageRef {
    pub message: QName,
    /// The input's, output's or fault's own name.
    pub name: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    pub name: String,
    pub input: Option<MessageRef>,
    pub output: Option<MessageRef>,
    pub faults: Vec<MessageRef>,
    pub documentation: Option<String>,
    pub span: Span,
}

/// An abstract interface: a named set of operations.
#[derive(Clone, Debug, PartialEq)]
pub struct PortType {
    pub name: QName,
    pub operations: Vec<Operation>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoapVersion {
    V1_1,
    V1_2,
}

impl SoapVersion {
    /// The media type SOAP messages of this version are sent with.
    pub fn media_type(self) -> &'static str {
        match self {
            SoapVersion::V1_1 => "text/xml",
            SoapVersion::V1_2 => "application/soap+xml",
        }
    }
}

/// How a SOAP body is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The body holds the parts' elements.
    Document,
    /// The body holds an element named after the operation, which holds an
    /// element per part.
    Rpc,
}

/// A SOAP header a binding adds to a message.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderRef {
    pub message: QName,
    pub part: String,
    pub span: Span,
}

/// How one direction of an operation is bound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundMessage {
    /// The parts in the body, or `None` for all of them.
    pub parts: Option<Vec<String>>,
    /// `use="encoded"` (SOAP encoding), compared as literal.
    pub encoded: bool,
    pub headers: Vec<HeaderRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoundOperation {
    pub name: String,
    pub soap_action: Option<String>,
    pub style: Option<Style>,
    pub input: BoundMessage,
    pub output: BoundMessage,
    pub span: Span,
}

/// A concrete protocol binding of a port type.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: QName,
    pub port_type: QName,
    /// `None` for bindings to other protocols (HTTP GET/POST, MIME).
    pub soap: Option<SoapVersion>,
    pub style: Style,
    pub operations: Vec<BoundOperation>,
    pub span: Span,
}

impl Binding {
    pub fn operation(&self, name: &str) -> Option<&BoundOperation> {
        self.operations.iter().find(|o| o.name == name)
    }
}

/// A port: a binding at an address.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    pub binding: QName,
    pub address: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub name: QName,
    pub ports: Vec<Port>,
    pub span: Span,
}

/// Everything a WSDL description and the descriptions it imports declare.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Definitions {
    pub target_namespace: Option<String>,
    pub messages: Vec<Message>,
    pub port_types: Vec<PortType>,
    pub bindings: Vec<Binding>,
    pub services: Vec<Service>,
}

impl Definitions {
    pub fn message(&self, name: &QName) -> Option<&Message> {
        self.messages.iter().find(|m| m.name == *name)
    }

    /// The bindings of `port_type`, in document order.
    pub fn bindings_of<'a>(&'a self, port_type: &'a QName) -> impl Iterator<Item = &'a Binding> {
        self.bindings
            .iter()
            .filter(move |b| b.port_type == *port_type)
    }
}
