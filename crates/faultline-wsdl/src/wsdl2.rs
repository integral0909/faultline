//! Reading WSDL 2.0 descriptions into the same [`Definitions`] as WSDL
//! 1.1, so that both are lowered alike.
//!
//! * An `interface` is a port type; `extends` brings in the operations of
//!   the interfaces it names.
//! * An operation's `input`, `output`, `infault` and `outfault` name
//!   elements directly. Each becomes a message with one part holding the
//!   element: `#any` (or `#other`) is any content, `#none` no content.
//! * An in-only operation has no output, so it is one-way.
//! * A SOAP binding (`type="http://www.w3.org/ns/wsdl/soap"`) is SOAP 1.2
//!   unless `wsoap:version="1.1"`, document style, with each operation's
//!   `wsoap:action`. HTTP bindings contribute no protocol details, as in
//!   WSDL 1.1.

use faultline_doc::UriRef;
use faultline_source::Span;
use faultline_xml::{Element, QName};
use faultline_xsd::XS;

use crate::codes;
use crate::model::{
    Binding, BoundMessage, BoundOperation, Message, MessageRef, Operation, Part, Port, PortType,
    Service, SoapVersion, Style,
};
use crate::reader::{Reader, WSDL_2};

/// The WSDL 2.0 SOAP binding extension's namespace, and binding type.
pub const WSOAP: &str = "http://www.w3.org/ns/wsdl/soap";

/// An interface's `extends`, resolved after every document is read.
#[derive(Clone, Debug)]
pub(crate) struct Extends {
    pub interface: QName,
    pub parents: Vec<(QName, Span)>,
}

/// An interface fault: the element it carries.
#[derive(Clone, Debug)]
pub(crate) struct InterfaceFault {
    pub name: QName,
    pub element: Option<Content>,
}

/// What a message label or fault refers to.
#[derive(Clone, Debug)]
pub(crate) enum Content {
    Element(QName, Span),
    Any(Span),
    None,
}

impl Reader<'_> {
    /// Reads a WSDL 2.0 `description`.
    pub(crate) fn read_description(&mut self, root: &Element, base: &UriRef) {
        let target = root.attr("targetNamespace").map(str::to_owned);
        if self.definitions.target_namespace.is_none() {
            self.definitions.target_namespace.clone_from(&target);
        }
        let name = |local: &str| QName::new(target.as_deref(), local);
        for child in root.elements() {
            if child.name.namespace.as_deref() != Some(WSDL_2) {
                continue;
            }
            match child.name.local.as_str() {
                "import" | "include" => self.import(child, base),
                "types" => {
                    for schema in child.children_named(XS, "schema") {
                        self.schemas.add_schema(schema, base);
                    }
                }
                "interface" => {
                    if let Some(local) = self.required(child, "name") {
                        self.interface(child, name(&local));
                    }
                }
                "binding" => {
                    if let Some(local) = self.required(child, "name")
                        && let Some(binding) = self.binding2(child, name(&local))
                    {
                        self.definitions.bindings.push(binding);
                    }
                }
                "service" => {
                    if let Some(local) = self.required(child, "name") {
                        let service = self.service2(child, name(&local));
                        self.definitions.services.push(service);
                    }
                }
                _ => {}
            }
        }
    }

    /// What an `element` attribute names.
    fn element_ref(&mut self, element: &Element) -> Content {
        let Some(attr) = element.attribute("element") else {
            return Content::Any(element.name_span);
        };
        match attr.value.as_str() {
            "#none" => Content::None,
            "#any" | "#other" => Content::Any(attr.value_span),
            _ => match self.qname(element, "element") {
                Some((name, span)) => Content::Element(name, span),
                None => Content::Any(attr.value_span),
            },
        }
    }

    /// A message with one part carrying `content`.
    fn synthetic_message(&mut self, name: QName, content: &Content, span: Span) -> QName {
        let part = |element: Option<(QName, Span)>, ty: Option<(QName, Span)>, span: Span| Part {
            name: "body".to_owned(),
            element,
            ty,
            span,
        };
        let parts = match content {
            Content::Element(element, at) => vec![part(Some((element.clone(), *at)), None, *at)],
            Content::Any(at) => {
                vec![part(
                    None,
                    Some((QName::new(Some(XS), "anyType"), *at)),
                    *at,
                )]
            }
            Content::None => Vec::new(),
        };
        self.definitions.messages.push(Message {
            name: name.clone(),
            parts,
            span,
        });
        name
    }

    fn interface(&mut self, element: &Element, name: QName) {
        let namespace = name.namespace.clone();
        let qualified = |local: String| QName::new(namespace.as_deref(), &local);
        for fault in element.children_named(WSDL_2, "fault") {
            if let Some(local) = self.required(fault, "name") {
                let content = self.element_ref(fault);
                self.interface_faults.push(InterfaceFault {
                    name: qualified(local),
                    element: Some(content),
                });
            }
        }
        let parents: Vec<(QName, Span)> = element
            .attribute("extends")
            .map(|attr| {
                attr.value
                    .split_whitespace()
                    .filter_map(|text| element.resolve_qname(text).map(|q| (q, attr.value_span)))
                    .collect()
            })
            .unwrap_or_default();
        if !parents.is_empty() {
            self.extends.push(Extends {
                interface: name.clone(),
                parents,
            });
        }
        let mut operations = Vec::new();
        for op in element.children_named(WSDL_2, "operation") {
            let Some(op_name) = self.required(op, "name") else {
                continue;
            };
            let mut direction = |child: &str, label: &str| {
                op.child(WSDL_2, child).map(|message| {
                    let content = self.element_ref(message);
                    let synthetic = qualified(format!("{}.{op_name}.{label}", name.local));
                    let message_name = self.synthetic_message(synthetic, &content, message.span);
                    MessageRef {
                        message: message_name,
                        name: message.attr("messageLabel").map(str::to_owned),
                        span: message.span,
                    }
                })
            };
            let input = direction("input", "In");
            let output = direction("output", "Out");
            let mut faults = Vec::new();
            for fault in op
                .elements()
                .filter(|e| e.name.namespace.as_deref() == Some(WSDL_2))
                .filter(|e| matches!(e.name.local.as_str(), "infault" | "outfault"))
            {
                let Some((reference, span)) = self.qname(fault, "ref") else {
                    continue;
                };
                faults.push(MessageRef {
                    message: fault_message_name(&reference),
                    name: Some(reference.local.clone()),
                    span,
                });
            }
            let documentation = op
                .child(WSDL_2, "documentation")
                .map(|d| d.text().trim().to_owned())
                .filter(|d| !d.is_empty());
            operations.push(Operation {
                name: op_name,
                input,
                output,
                faults,
                documentation,
                span: op.span,
            });
        }
        self.definitions.port_types.push(PortType {
            name,
            operations,
            span: element.span,
        });
    }

    fn binding2(&mut self, element: &Element, name: QName) -> Option<Binding> {
        let Some((interface, _)) = self.qname(element, "interface") else {
            self.error(
                codes::INVALID,
                "a binding names the interface it binds".into(),
                element.name_span,
            );
            return None;
        };
        let soap = (element.attr("type") == Some(WSOAP)).then(|| {
            match namespaced(element, WSOAP, "version") {
                Some("1.1") => SoapVersion::V1_1,
                _ => SoapVersion::V1_2,
            }
        });
        let mut operations = Vec::new();
        for op in element.children_named(WSDL_2, "operation") {
            let Some((reference, _)) = self.qname(op, "ref") else {
                continue;
            };
            operations.push(BoundOperation {
                name: reference.local,
                soap_action: namespaced(op, WSOAP, "action").map(str::to_owned),
                style: None,
                input: BoundMessage::default(),
                output: BoundMessage::default(),
                span: op.span,
            });
        }
        Some(Binding {
            name,
            port_type: interface,
            soap,
            style: Style::Document,
            operations,
            span: element.span,
        })
    }

    fn service2(&mut self, element: &Element, name: QName) -> Service {
        let mut ports = Vec::new();
        for endpoint in element.children_named(WSDL_2, "endpoint") {
            let Some(endpoint_name) = self.required(endpoint, "name") else {
                continue;
            };
            let Some((binding, _)) = self.qname(endpoint, "binding") else {
                continue;
            };
            ports.push(Port {
                name: endpoint_name,
                binding,
                address: endpoint.attr("address").map(str::to_owned),
                span: endpoint.span,
            });
        }
        Service {
            name,
            ports,
            span: element.span,
        }
    }

    /// Completes WSDL 2.0 descriptions once every document is read: fault
    /// messages for interface faults, and inherited operations.
    pub(crate) fn finish_wsdl2(&mut self) {
        for fault in std::mem::take(&mut self.interface_faults) {
            let content = fault.element.unwrap_or(Content::None);
            let span = match &content {
                Content::Element(_, span) | Content::Any(span) => *span,
                Content::None => Span::empty(faultline_source::SourceId::from_raw(0), 0),
            };
            self.synthetic_message(fault_message_name(&fault.name), &content, span);
        }
        let extends = std::mem::take(&mut self.extends);
        for _ in 0..extends.len() {
            let mut changed = false;
            for entry in &extends {
                let inherited: Vec<Operation> = entry
                    .parents
                    .iter()
                    .filter_map(|(parent, _)| self.definitions.port_type(parent))
                    .flat_map(|p| p.operations.clone())
                    .collect();
                let Some(port_type) = self
                    .definitions
                    .port_types
                    .iter_mut()
                    .find(|p| p.name == entry.interface)
                else {
                    continue;
                };
                for op in inherited {
                    if !port_type.operations.iter().any(|o| o.name == op.name) {
                        port_type.operations.push(op);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for entry in &extends {
            for (parent, span) in &entry.parents {
                if self.definitions.port_type(parent).is_none() {
                    self.error(
                        codes::UNRESOLVED,
                        format!("interface {} is not defined", parent.local),
                        *span,
                    );
                }
            }
        }
    }
}

/// The message an interface fault's content is carried in.
fn fault_message_name(fault: &QName) -> QName {
    QName::new(fault.namespace.as_deref(), format!("fault.{}", fault.local))
}

/// The value of the attribute `local` in `namespace`.
fn namespaced<'e>(element: &'e Element, namespace: &str, local: &str) -> Option<&'e str> {
    element
        .attributes
        .iter()
        .find(|a| a.name.namespace.as_deref() == Some(namespace) && a.name.local == local)
        .map(|a| a.value.as_str())
}
