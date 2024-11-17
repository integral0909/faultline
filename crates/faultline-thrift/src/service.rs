//! Lowering Thrift services into RPC operations.
//!
//! Each function of each service in the entry file is an operation
//! (`op[Pets/get]`), including the functions the service inherits through
//! `extends`, which a client of the service calls the same way. On the
//! wire a call is:
//!
//! * a request carrying the arguments as a struct, compared like one: an
//!   argument is identified by its id;
//! * a reply carrying a result struct in which exactly one field is set:
//!   `success` (field 0) with the return value, or one of the declared
//!   exceptions under its id. The result is closed: a client that reads a
//!   field it does not know, such as a new exception, fails the call, so
//!   a new exception is a risk for clients.
//!
//! A `oneway` function has no reply.

use std::collections::HashSet;

use faultline_ir::{
    Content, ExclusiveGroup, Field, Meta, ObjectType, Operation, OperationKey, RequestBody,
    Response, StatusKey, TypeId, TypeKind, TypeNode,
};

use crate::ast::{Definition, Function, Requiredness, Service};
use crate::linker::DefId;
use crate::lower::{Lowerer, meta};

/// The media type recorded for Thrift request and reply bodies.
pub const THRIFT_MEDIA_TYPE: &str = "application/x-thrift";

impl Lowerer<'_> {
    /// Lowers the services of the entry file.
    pub(crate) fn services(&mut self) {
        let definitions = &self.files[0].document.definitions;
        for (index, definition) in definitions.iter().enumerate() {
            if let Definition::Service(service) = definition {
                self.service(DefId { file: 0, index }, service);
            }
        }
    }

    fn service(&mut self, id: DefId, service: &Service) {
        let name = service.name.text.clone();
        let mut services = HashSet::new();
        let mut functions = HashSet::new();
        // The service's own functions, then those it inherits, nearest
        // first; of two functions with one name, the nearer is called.
        let mut current = Some((id, service));
        while let Some((id, service)) = current.take() {
            if !services.insert(id) {
                break;
            }
            for function in &service.functions {
                if functions.insert(function.name.text.as_str()) {
                    let op = self.function(&name, id.file, service, function);
                    self.api.operations.push(op);
                }
            }
            current = service.extends.as_ref().and_then(|parent| {
                let parent = self.linked.resolve(id.file, &parent.text)?;
                match self.definition(parent) {
                    Definition::Service(s) => Some((parent, s)),
                    _ => None,
                }
            });
        }
    }

    fn function(
        &mut self,
        service: &str,
        file: usize,
        owner: &Service,
        function: &Function,
    ) -> Operation {
        let mut op = Operation::new(OperationKey::Rpc {
            service: service.to_owned(),
            method: function.name.text.clone(),
        });
        let arguments = self.arguments(file, &function.arguments);
        op.request_body = Some(RequestBody {
            required: true,
            content: vec![Content {
                media_type: THRIFT_MEDIA_TYPE.to_owned(),
                ty: arguments,
                origin: Some(function.name.span),
            }],
            meta: Meta::default(),
            origin: Some(function.name.span),
        });
        if !function.oneway {
            let result = self.result(file, function);
            op.responses.push(Response {
                status: StatusKey::Default,
                headers: Vec::new(),
                content: vec![Content {
                    media_type: THRIFT_MEDIA_TYPE.to_owned(),
                    ty: result,
                    origin: Some(function.return_span),
                }],
                meta: Meta::default(),
                origin: Some(function.return_span),
            });
        }
        op.meta = meta(function.doc.as_ref(), &function.annotations);
        op.meta.deprecated |= meta(None, &owner.annotations).deprecated;
        op.origin = Some(function.name.span);
        op
    }

    /// The arguments struct: open, as servers skip arguments they do not
    /// know.
    fn arguments(&mut self, file: usize, arguments: &[crate::ast::Field]) -> TypeId {
        let any = self.api.types.alloc(TypeNode::new(TypeKind::Any));
        let mut object = ObjectType::new(any);
        for argument in arguments {
            let ty = self.field_type(file, &argument.ty);
            let mut field = Field::new(
                argument.name.text.clone(),
                ty,
                argument.requiredness == Requiredness::Required,
            );
            field.tag = crate::lower::tag(argument.id);
            field.meta = meta(argument.doc.as_ref(), &argument.annotations);
            field.origin = Some(argument.span);
            object.fields.push(field);
        }
        self.api
            .types
            .alloc(TypeNode::new(TypeKind::Object(object)))
    }

    /// The result struct: `success` or one exception, and nothing else.
    fn result(&mut self, file: usize, function: &Function) -> TypeId {
        let never = self.api.types.alloc(TypeNode::new(TypeKind::Never));
        let mut object = ObjectType::new(never);
        if let Some(returns) = &function.returns {
            let ty = self.field_type(file, returns);
            let mut success = Field::new("success", ty, false);
            success.tag = Some(0);
            success.origin = Some(returns.span);
            object.fields.push(success);
        }
        for thrown in &function.throws {
            let ty = self.field_type(file, &thrown.ty);
            let mut field = Field::new(thrown.name.text.clone(), ty, false);
            field.tag = crate::lower::tag(thrown.id);
            field.meta = meta(thrown.doc.as_ref(), &thrown.annotations);
            field.origin = Some(thrown.span);
            object.fields.push(field);
        }
        // A void function's reply may carry nothing at all.
        if function.returns.is_some() {
            object.min_properties = 1;
        }
        if !object.fields.is_empty() {
            object.exclusive.push(ExclusiveGroup {
                name: "result".to_owned(),
                fields: object.fields.iter().map(|f| f.name.clone()).collect(),
                origin: None,
            });
        }
        self.api
            .types
            .alloc(TypeNode::new(TypeKind::Object(object)))
    }
}
