#![allow(dead_code, unreachable_pub)] // shared by several test crates; each uses a subset

use faultline_ir::{
    Access, Api, ApiFormat, ArrayType, Callback, Content, Field, HttpMethod, Meta, ObjectType,
    Operation, OperationKey, ParamLocation, Parameter, PathTemplate, QualName, RequestBody,
    Response, StatusKey, StringType, TypeId, TypeKind, TypeNode,
};

/// Terse construction of IR graphs for tests.
pub struct Builder {
    pub api: Api,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            api: Api::new("test", ApiFormat::OpenApi31),
        }
    }

    pub fn kind(&mut self, kind: TypeKind) -> TypeId {
        self.api.types.alloc_kind(kind)
    }

    pub fn string(&mut self) -> TypeId {
        self.kind(TypeKind::String(StringType::default()))
    }

    pub fn any(&mut self) -> TypeId {
        self.kind(TypeKind::Any)
    }

    pub fn array(&mut self, items: TypeId) -> TypeId {
        self.kind(TypeKind::Array(ArrayType::of(items)))
    }

    /// An open object with the given `(name, type, required)` fields.
    pub fn object(&mut self, fields: &[(&str, TypeId, bool)]) -> TypeId {
        let additional = self.any();
        let mut obj = ObjectType::new(additional);
        obj.fields = fields
            .iter()
            .map(|&(name, ty, required)| Field::new(name, ty, required))
            .collect();
        self.kind(TypeKind::Object(obj))
    }

    pub fn with_access(&mut self, id: TypeId, access: Access) -> TypeId {
        self.api.types.get_mut(id).meta.access = access;
        id
    }

    /// Reserves a named type; define it with [`define_named`](Self::define_named).
    pub fn reserve_named(&mut self, name: &str) -> TypeId {
        let id = self.api.types.reserve();
        self.api.named.insert(QualName::new(name), id);
        id
    }

    pub fn define_named(&mut self, id: TypeId, name: &str, kind: TypeKind) {
        self.api
            .types
            .define(id, TypeNode::new(kind).with_name(QualName::new(name)));
    }

    pub fn named(&mut self, name: &str, kind: TypeKind) -> TypeId {
        let id = self.reserve_named(name);
        self.define_named(id, name, kind);
        id
    }

    pub fn reference(&mut self, target: TypeId) -> TypeId {
        self.kind(TypeKind::Ref(target))
    }
}

pub fn http(method: &str, path: &str) -> Operation {
    Operation::new(OperationKey::Http {
        method: HttpMethod::parse(method).unwrap(),
        path: PathTemplate::parse(path),
    })
}

pub fn param(location: ParamLocation, name: &str, ty: TypeId) -> Parameter {
    Parameter {
        location,
        name: name.into(),
        required: false,
        ty,
        meta: Meta::default(),
        origin: None,
    }
}

pub fn json(ty: TypeId) -> Content {
    Content {
        media_type: "application/json".into(),
        ty,
        origin: None,
    }
}

pub fn request(ty: TypeId) -> RequestBody {
    RequestBody {
        required: true,
        content: vec![json(ty)],
        meta: Meta::default(),
        origin: None,
    }
}

pub fn response(status: &str, ty: TypeId) -> Response {
    Response {
        status: StatusKey::parse(status).unwrap(),
        headers: Vec::new(),
        content: vec![json(ty)],
        meta: Meta::default(),
        origin: None,
    }
}

pub fn callback(name: &str, operations: Vec<Operation>) -> Callback {
    Callback {
        name: name.into(),
        operations,
        origin: None,
    }
}
