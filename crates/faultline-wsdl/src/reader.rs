//! Reading WSDL 1.1 documents into [`Definitions`], and their schemas into
//! XML Schema components.

use std::collections::HashSet;

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Limits, SourceMap, Span};
use faultline_xml::{Element, QName};
use faultline_xsd::XS;

use crate::codes;
use crate::model::{
    Binding, BoundMessage, BoundOperation, Definitions, HeaderRef, Message, MessageRef, Operation,
    Part, Port, PortType, Service, SoapVersion, Style,
};

/// The WSDL 1.1 namespace.
pub const WSDL: &str = "http://schemas.xmlsoap.org/wsdl/";
/// The WSDL 2.0 namespace, recognized to say it is not supported.
pub const WSDL_2: &str = "http://www.w3.org/ns/wsdl";
/// The SOAP 1.1 binding namespace.
pub const SOAP_11: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
/// The SOAP 1.2 binding namespace.
pub const SOAP_12: &str = "http://schemas.xmlsoap.org/wsdl/soap12/";

/// Reads WSDL documents, following `wsdl:import`, and collects their
/// definitions and embedded or imported schemas.
pub struct Reader<'a> {
    pub schemas: faultline_xsd::Reader<'a>,
    pub definitions: Definitions,
    pub diagnostics: Diagnostics,
    loaded: HashSet<String>,
}

impl std::fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl<'a> Reader<'a> {
    pub fn new(loader: &'a dyn Loader, sources: &'a SourceMap, limits: Limits) -> Self {
        Self {
            schemas: faultline_xsd::Reader::new(loader, sources, limits),
            definitions: Definitions::default(),
            diagnostics: Diagnostics::new(),
            loaded: HashSet::new(),
        }
    }

    fn error(&mut self, code: DiagnosticCode, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    /// Reads the description at `uri` and what it imports.
    pub fn load(&mut self, uri: &UriRef) {
        self.load_at(uri, None);
    }

    fn load_at(&mut self, uri: &UriRef, at: Option<Span>) {
        if !self
            .loaded
            .insert(uri.without_fragment().normalized().to_string())
        {
            return;
        }
        let Some(root) = self.schemas.read_document(uri, at) else {
            return;
        };
        if root.name.is(XS, "schema") {
            // `wsdl:import` may name a schema document.
            self.schemas.add_schema(&root, uri);
        } else if root.name.is(WSDL, "definitions") {
            self.read_definitions(&root, uri);
        } else if root.name.namespace.as_deref() == Some(WSDL_2) {
            self.error(
                codes::UNSUPPORTED_VERSION,
                "WSDL 2.0 descriptions are not supported; WSDL 1.1 is".into(),
                root.name_span,
            );
        } else {
            self.error(
                codes::NOT_WSDL,
                format!(
                    "{uri} is not a WSDL description: its root is <{}>",
                    root.raw_name
                ),
                root.name_span,
            );
        }
    }

    fn read_definitions(&mut self, root: &Element, base: &UriRef) {
        let target = root.attr("targetNamespace").map(str::to_owned);
        if self.definitions.target_namespace.is_none() {
            self.definitions.target_namespace.clone_from(&target);
        }
        let name = |local: &str| QName::new(target.as_deref(), local);
        for child in root.elements() {
            if child.name.namespace.as_deref() != Some(WSDL) {
                continue;
            }
            match child.name.local.as_str() {
                "import" => self.import(child, base),
                "types" => {
                    for schema in child.children_named(XS, "schema") {
                        self.schemas.add_schema(schema, base);
                    }
                }
                "message" => {
                    let Some(local) = self.required(child, "name") else {
                        continue;
                    };
                    let message = self.message(child, name(&local));
                    self.definitions.messages.push(message);
                }
                "portType" => {
                    let Some(local) = self.required(child, "name") else {
                        continue;
                    };
                    let port_type = self.port_type(child, name(&local));
                    self.definitions.port_types.push(port_type);
                }
                "binding" => {
                    let Some(local) = self.required(child, "name") else {
                        continue;
                    };
                    if let Some(binding) = self.binding(child, name(&local)) {
                        self.definitions.bindings.push(binding);
                    }
                }
                "service" => {
                    let Some(local) = self.required(child, "name") else {
                        continue;
                    };
                    let service = self.service(child, name(&local));
                    self.definitions.services.push(service);
                }
                _ => {}
            }
        }
    }

    fn import(&mut self, element: &Element, base: &UriRef) {
        let Some(attribute) = element.attribute("location") else {
            return;
        };
        let span = attribute.value_span;
        let Ok(reference) = UriRef::parse(&attribute.value) else {
            self.error(
                codes::LOAD,
                "location is not a valid URI reference".into(),
                span,
            );
            return;
        };
        let is_file = base
            .scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"));
        let target = if is_file && !reference.has_scheme() {
            let Some(target) = base.resolve_strict(&reference) else {
                self.error(
                    codes::LOAD,
                    "location points outside the workspace root".into(),
                    span,
                );
                return;
            };
            target
        } else {
            base.resolve(&reference)
        };
        self.load_at(&target, Some(span));
    }

    fn required(&mut self, element: &Element, attribute: &str) -> Option<String> {
        if let Some(value) = element.attr(attribute) {
            return Some(value.to_owned());
        }
        self.error(
            codes::INVALID,
            format!("<{}> needs a {attribute}", element.raw_name),
            element.name_span,
        );
        None
    }

    /// A `QName`-valued attribute.
    fn qname(&mut self, element: &Element, attribute: &str) -> Option<(QName, Span)> {
        let attr = element.attribute(attribute)?;
        if let Some(name) = element.resolve_qname(&attr.value) {
            return Some((name, attr.value_span));
        }
        let span = attr.value_span;
        let prefix = attr.value.split(':').next().unwrap_or_default().to_owned();
        self.error(
            codes::INVALID,
            format!("namespace prefix `{prefix}` is not declared"),
            span,
        );
        None
    }

    fn message(&mut self, element: &Element, name: QName) -> Message {
        let mut parts = Vec::new();
        for part in element.children_named(WSDL, "part") {
            let Some(part_name) = self.required(part, "name") else {
                continue;
            };
            let element_ref = self.qname(part, "element");
            let ty = self.qname(part, "type");
            if element_ref.is_none() && ty.is_none() {
                self.error(
                    codes::INVALID,
                    format!("part {part_name} needs an element or a type"),
                    part.name_span,
                );
            }
            parts.push(Part {
                name: part_name,
                element: element_ref,
                ty,
                span: part.span,
            });
        }
        Message {
            name,
            parts,
            span: element.span,
        }
    }

    fn message_ref(&mut self, element: &Element) -> Option<MessageRef> {
        let (message, span) = self.qname(element, "message")?;
        Some(MessageRef {
            message,
            name: element.attr("name").map(str::to_owned),
            span,
        })
    }

    fn port_type(&mut self, element: &Element, name: QName) -> PortType {
        let mut operations = Vec::new();
        for op in element.children_named(WSDL, "operation") {
            let Some(op_name) = self.required(op, "name") else {
                continue;
            };
            let input = op.child(WSDL, "input").and_then(|i| self.message_ref(i));
            let output = op.child(WSDL, "output").and_then(|o| self.message_ref(o));
            let faults = op
                .children_named(WSDL, "fault")
                .filter_map(|f| self.message_ref(f))
                .collect();
            let documentation = op
                .child(WSDL, "documentation")
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
        PortType {
            name,
            operations,
            span: element.span,
        }
    }

    fn binding(&mut self, element: &Element, name: QName) -> Option<Binding> {
        if element.attribute("type").is_none() {
            self.error(
                codes::INVALID,
                "a binding needs the type it binds".into(),
                element.name_span,
            );
            return None;
        }
        let (port_type, _) = self.qname(element, "type")?;
        let soap = element
            .child(SOAP_11, "binding")
            .map(|b| (SoapVersion::V1_1, b))
            .or_else(|| {
                element
                    .child(SOAP_12, "binding")
                    .map(|b| (SoapVersion::V1_2, b))
            });
        let style = soap
            .and_then(|(_, b)| b.attr("style"))
            .map_or(Style::Document, parse_style);
        let namespace = soap.map(|(version, _)| match version {
            SoapVersion::V1_1 => SOAP_11,
            SoapVersion::V1_2 => SOAP_12,
        });
        let mut operations = Vec::new();
        for op in element.children_named(WSDL, "operation") {
            let Some(op_name) = self.required(op, "name") else {
                continue;
            };
            let soap_operation = namespace.and_then(|ns| op.child(ns, "operation"));
            let soap_action = soap_operation
                .and_then(|o| o.attr("soapAction"))
                .map(str::to_owned)
                .filter(|a| !a.is_empty());
            let op_style = soap_operation
                .and_then(|o| o.attr("style"))
                .map(parse_style);
            let input = op
                .child(WSDL, "input")
                .map(|i| self.bound_message(i, namespace))
                .unwrap_or_default();
            let output = op
                .child(WSDL, "output")
                .map(|o| self.bound_message(o, namespace))
                .unwrap_or_default();
            operations.push(BoundOperation {
                name: op_name,
                soap_action,
                style: op_style,
                input,
                output,
                span: op.span,
            });
        }
        Some(Binding {
            name,
            port_type,
            soap: soap.map(|(version, _)| version),
            style,
            operations,
            span: element.span,
        })
    }

    fn bound_message(&mut self, element: &Element, namespace: Option<&str>) -> BoundMessage {
        let Some(namespace) = namespace else {
            return BoundMessage::default();
        };
        let body = element.child(namespace, "body");
        let parts = body
            .and_then(|b| b.attr("parts"))
            .map(|p| p.split_whitespace().map(str::to_owned).collect());
        let encoded = body.and_then(|b| b.attr("use")) == Some("encoded");
        if encoded && let Some(body) = body {
            self.diagnostics.push(
                Diagnostic::warning(
                    "SOAP encoding is compared as if the parts were literal".to_owned(),
                )
                .with_code(codes::ENCODED)
                .with_primary(body.name_span, ""),
            );
        }
        let mut headers = Vec::new();
        for header in element.children_named(namespace, "header") {
            let Some((message, span)) = self.qname(header, "message") else {
                continue;
            };
            let Some(part) = self.required(header, "part") else {
                continue;
            };
            headers.push(HeaderRef {
                message,
                part,
                span,
            });
        }
        BoundMessage {
            parts,
            encoded,
            headers,
        }
    }

    fn service(&mut self, element: &Element, name: QName) -> Service {
        let mut ports = Vec::new();
        for port in element.children_named(WSDL, "port") {
            let Some(port_name) = self.required(port, "name") else {
                continue;
            };
            let Some((binding, _)) = self.qname(port, "binding") else {
                continue;
            };
            let address = port
                .elements()
                .find(|e| e.name.local == "address")
                .and_then(|a| a.attr("location"))
                .map(str::to_owned);
            ports.push(Port {
                name: port_name,
                binding,
                address,
                span: port.span,
            });
        }
        Service {
            name,
            ports,
            span: element.span,
        }
    }
}

fn parse_style(text: &str) -> Style {
    if text == "rpc" {
        Style::Rpc
    } else {
        Style::Document
    }
}
