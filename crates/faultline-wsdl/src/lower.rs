//! Lowering WSDL descriptions into the IR.
//!
//! Each operation of each port type becomes an operation keyed by port
//! type and operation name (`PetPortType/GetPet`):
//!
//! * the input message is the request body and the output message the
//!   `200` response, in the SOAP body layout the binding's style gives: a
//!   document-style body is its part's element (or an object of several),
//!   an RPC-style body an element named after the operation (`GetPet`, and
//!   `GetPetResponse` for the output) holding one member per part;
//! * each SOAP binding contributes its media type (`text/xml` for 1.1,
//!   `application/soap+xml` for 1.2), so dropping a binding removes a media
//!   type clients use;
//! * the SOAP action is where each version sends it: for SOAP 1.1 a
//!   required `SOAPAction` header whose value must be the action, for SOAP
//!   1.2 the `action` parameter of the media type. Changing it breaks
//!   clients that send the old one;
//! * SOAP headers are header parameters (input) and response headers
//!   (output);
//! * faults are the `500` response, a union of one member per fault named
//!   after it, so a new fault is a variant clients may not handle.

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_ir::{
    Api, ApiFormat, Content, EnumType, EnumValue, Field, Header, Literal, Meta, ObjectType,
    Operation, OperationKey, ParamLocation, Parameter, RequestBody, Response, Retired, StatusKey,
    TypeId, TypeKind, TypeNode, UnionMode, UnionType,
};
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap, Span};
use faultline_xml::QName;
use faultline_xsd::model::TypeRef;

use crate::codes;
use crate::model::{
    Binding, BoundMessage, Definitions, MessageRef, Operation as WsdlOperation, PortType,
    SoapVersion, Style,
};
use crate::reader::Reader;

/// The media type of messages without a SOAP binding.
const DEFAULT_MEDIA_TYPE: &str = "text/xml";

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the description has errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Reads the WSDL description at `entry` with what it imports, and lowers
/// its port types into an [`Api`] named `name`.
pub fn lower_wsdl(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    limits: Limits,
) -> Lowered {
    let mut reader = Reader::new(loader, sources, limits);
    reader.load(entry);
    let mut diagnostics = std::mem::take(&mut reader.diagnostics);
    diagnostics.extend(std::mem::take(&mut reader.schemas.diagnostics));
    if diagnostics.has_errors() {
        diagnostics.sort();
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut lowering = Lowering {
        schemas: faultline_xsd::Lowerer::new(&reader.schemas.components, name),
        definitions: &reader.definitions,
        diagnostics: Diagnostics::new(),
    };
    lowering.schemas.api.format = ApiFormat::Wsdl;
    for port_type in &reader.definitions.port_types {
        lowering.port_type(port_type);
    }
    diagnostics.extend(lowering.diagnostics);
    diagnostics.extend(std::mem::take(&mut lowering.schemas.diagnostics));
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(lowering.schemas.api);
    Lowered { api, diagnostics }
}

struct Lowering<'c> {
    schemas: faultline_xsd::Lowerer<'c>,
    definitions: &'c Definitions,
    diagnostics: Diagnostics,
}

/// Which way a message goes, for its RPC wrapper's name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Input,
    Output,
}

impl Lowering<'_> {
    fn error(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::UNRESOLVED)
                .with_primary(span, ""),
        );
    }

    fn alloc(&mut self, kind: TypeKind, span: Option<Span>) -> TypeId {
        let mut node = TypeNode::new(kind);
        node.origin = span;
        self.schemas.api.types.alloc(node)
    }

    fn port_type(&mut self, port_type: &PortType) {
        let bindings: Vec<&Binding> = self.definitions.bindings_of(&port_type.name).collect();
        for operation in &port_type.operations {
            let op = self.operation(port_type, operation, &bindings);
            self.schemas.api.operations.push(op);
        }
    }

    fn operation(
        &mut self,
        port_type: &PortType,
        operation: &WsdlOperation,
        bindings: &[&Binding],
    ) -> Operation {
        let mut op = Operation::new(OperationKey::Rpc {
            service: port_type.name.local.clone(),
            method: operation.name.clone(),
        });
        op.origin = Some(operation.span);
        op.meta.description.clone_from(&operation.documentation);

        let bound: Vec<_> = bindings
            .iter()
            .filter_map(|b| b.operation(&operation.name).map(|o| (*b, o)))
            .collect();
        let media_types = media_types(bindings, &operation.name);
        let style = bound
            .first()
            .map_or(Style::Document, |(b, o)| o.style.unwrap_or(b.style));
        let input_binding = bound
            .first()
            .map(|(_, o)| o.input.clone())
            .unwrap_or_default();
        let output_binding = bound
            .first()
            .map(|(_, o)| o.output.clone())
            .unwrap_or_default();

        op.parameters = self.request_headers(&bound, &input_binding, operation.span);
        if let Some(input) = &operation.input
            && let Some(ty) = self.body(
                input,
                &input_binding,
                style,
                &operation.name,
                Direction::Input,
            )
        {
            op.request_body = Some(RequestBody {
                required: true,
                content: contents(&media_types, ty, input.span),
                meta: Meta::default(),
                origin: Some(input.span),
            });
        }
        if let Some(output) = &operation.output
            && let Some(ty) = self.body(
                output,
                &output_binding,
                style,
                &operation.name,
                Direction::Output,
            )
        {
            let mut headers = Vec::new();
            for header in &output_binding.headers {
                if let Some((name, ty)) =
                    self.header_part(&header.message, &header.part, header.span)
                {
                    headers.push(Header {
                        name,
                        required: true,
                        ty,
                        meta: Meta::default(),
                        origin: Some(header.span),
                    });
                }
            }
            op.responses.push(Response {
                status: StatusKey::Code(200),
                headers,
                content: contents(&media_types, ty, output.span),
                meta: Meta::default(),
                origin: Some(output.span),
            });
        }
        if !operation.faults.is_empty() {
            let ty = self.faults(&operation.faults);
            let span = operation.faults[0].span;
            op.responses.push(Response {
                status: StatusKey::Code(500),
                headers: Vec::new(),
                content: contents(&media_types, ty, span),
                meta: Meta::default(),
                origin: Some(span),
            });
        }
        op
    }

    /// The SOAP 1.1 `SOAPAction` header and the SOAP headers of an input.
    fn request_headers(
        &mut self,
        bound: &[(&Binding, &crate::model::BoundOperation)],
        input: &BoundMessage,
        span: Span,
    ) -> Vec<Parameter> {
        let mut parameters = Vec::new();
        let actions: Vec<String> = bound
            .iter()
            .filter(|(b, _)| b.soap == Some(SoapVersion::V1_1))
            .filter_map(|(_, o)| o.soap_action.clone())
            .fold(Vec::new(), |mut all, action| {
                if !all.contains(&action) {
                    all.push(action);
                }
                all
            });
        if !actions.is_empty() {
            let ty = self.one_of(&actions, span);
            parameters.push(Parameter {
                location: ParamLocation::Header,
                name: "SOAPAction".to_owned(),
                required: true,
                ty,
                serialization: None,
                meta: Meta::default(),
                origin: bound.first().map(|(_, o)| o.span),
            });
        }
        for header in &input.headers {
            if let Some((name, ty)) = self.header_part(&header.message, &header.part, header.span) {
                parameters.push(Parameter {
                    location: ParamLocation::Header,
                    name,
                    required: true,
                    ty,
                    serialization: None,
                    meta: Meta::default(),
                    origin: Some(header.span),
                });
            }
        }

        parameters
    }

    /// A string that must be one of `values`.
    fn one_of(&mut self, values: &[String], span: Span) -> TypeId {
        self.alloc(
            TypeKind::Enum(EnumType {
                values: values
                    .iter()
                    .map(|v| EnumValue::new(Literal::string(v.clone())))
                    .collect(),
                open: false,
                retired: Retired::default(),
                writer_exact: false,
            }),
            Some(span),
        )
    }

    /// The type of one part: its element's or its type's.
    fn part_type(&mut self, part: &crate::model::Part) -> TypeId {
        if let Some((element, span)) = &part.element {
            if let Some(decl) = self.schemas.components_element(element) {
                return self.schemas.element_type(&decl);
            }
            self.error(format!("element {element} is not declared"), *span);
            return self.schemas.api.types.alloc_kind(TypeKind::Any);
        }
        match &part.ty {
            Some((ty, span)) => self.schemas.type_ref(&TypeRef::Named(ty.clone(), *span)),
            None => self.schemas.api.types.alloc_kind(TypeKind::Any),
        }
    }

    /// The name a part has in a body or header: its element's local name,
    /// or the part's own name.
    fn part_name(part: &crate::model::Part) -> String {
        part.element
            .as_ref()
            .map_or_else(|| part.name.clone(), |(e, _)| e.local.clone())
    }

    fn header_part(&mut self, message: &QName, part: &str, span: Span) -> Option<(String, TypeId)> {
        let definitions = self.definitions;
        let Some(message) = definitions.message(message) else {
            self.error(format!("message {message} is not declared"), span);
            return None;
        };
        let Some(part) = message.parts.iter().find(|p| p.name == part) else {
            self.error(format!("message {} has no part {part}", message.name), span);
            return None;
        };
        Some((Self::part_name(part), self.part_type(part)))
    }

    /// The type of a message's SOAP body.
    fn body(
        &mut self,
        reference: &MessageRef,
        binding: &BoundMessage,
        style: Style,
        operation: &str,
        direction: Direction,
    ) -> Option<TypeId> {
        let definitions = self.definitions;
        let Some(message) = definitions.message(&reference.message) else {
            self.error(
                format!("message {} is not declared", reference.message),
                reference.span,
            );
            return None;
        };
        // Header parts are not in the body; `parts` names those that are.
        let in_headers: Vec<&str> = binding
            .headers
            .iter()
            .filter(|h| h.message == message.name)
            .map(|h| h.part.as_str())
            .collect();
        let parts: Vec<&crate::model::Part> = message
            .parts
            .iter()
            .filter(|p| {
                binding
                    .parts
                    .as_ref()
                    .is_none_or(|names| names.contains(&p.name))
            })
            .filter(|p| !in_headers.contains(&p.name.as_str()))
            .collect();
        if style == Style::Document
            && let [part] = parts.as_slice()
            && part.element.is_some()
        {
            return Some(self.part_type(part));
        }
        let never = self.alloc(TypeKind::Never, None);
        let mut object = ObjectType::new(never);
        for part in &parts {
            let ty = self.part_type(part);
            let mut field = Field::new(Self::part_name(part), ty, true);
            field.origin = Some(part.span);
            object.fields.push(field);
        }
        let body = self.alloc(TypeKind::Object(object), Some(message.span));
        if style == Style::Document {
            return Some(body);
        }
        // RPC style wraps the parts in an element named after the operation.
        let wrapper = match direction {
            Direction::Input => operation.to_owned(),
            Direction::Output => format!("{operation}Response"),
        };
        let never = self.alloc(TypeKind::Never, None);
        let mut outer = ObjectType::new(never);
        outer.fields.push(Field::new(wrapper, body, true));
        Some(self.alloc(TypeKind::Object(outer), Some(message.span)))
    }

    /// The faults, as a union of one-member objects named after each fault.
    fn faults(&mut self, faults: &[MessageRef]) -> TypeId {
        let mut variants = Vec::new();
        for fault in faults {
            let definitions = self.definitions;
            let Some(message) = definitions.message(&fault.message) else {
                self.error(
                    format!("message {} is not declared", fault.message),
                    fault.span,
                );
                continue;
            };
            let detail = match message.parts.as_slice() {
                [part] => self.part_type(part),
                _ => self.schemas.api.types.alloc_kind(TypeKind::Any),
            };
            let name = fault
                .name
                .clone()
                .unwrap_or_else(|| message.name.local.clone());
            let never = self.alloc(TypeKind::Never, None);
            let mut object = ObjectType::new(never);
            object.fields.push(Field::new(name, detail, true));
            variants.push(self.alloc(TypeKind::Object(object), Some(fault.span)));
        }
        self.alloc(
            TypeKind::Union(UnionType {
                mode: UnionMode::Tagged,
                variants,
                discriminator: None,
            }),
            faults.first().map(|f| f.span),
        )
    }
}

/// The media types of the SOAP bindings of `operation`, in document order.
/// SOAP 1.2 carries the action in its media type.
fn media_types(bindings: &[&Binding], operation: &str) -> Vec<String> {
    let mut media_types: Vec<String> = Vec::new();
    for binding in bindings {
        let Some(version) = binding.soap else {
            continue;
        };
        let action = binding
            .operation(operation)
            .and_then(|o| o.soap_action.as_deref())
            .filter(|_| version == SoapVersion::V1_2);
        let media_type = match action {
            Some(action) => format!("{}; action=\"{action}\"", version.media_type()),
            None => version.media_type().to_owned(),
        };
        if !media_types.contains(&media_type) {
            media_types.push(media_type);
        }
    }
    if media_types.is_empty() {
        media_types.push(DEFAULT_MEDIA_TYPE.to_owned());
    }
    media_types
}

fn contents(media_types: &[String], ty: TypeId, span: Span) -> Vec<Content> {
    media_types
        .iter()
        .map(|media_type| Content {
            media_type: media_type.clone(),
            ty,
            origin: Some(span),
        })
        .collect()
}
