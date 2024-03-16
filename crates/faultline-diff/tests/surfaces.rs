//! Diffs of API surfaces built directly in the IR.

use faultline_diff::{ChangeKind, diff};
use faultline_ir::{
    Api, ApiFormat, Callback, Content, HttpMethod, Meta, Operation, OperationKey, ParamLocation,
    Parameter, PathTemplate, Polarity, RequestBody, Response, StatusKey, StringType, TypeId,
    TypeKind,
};

fn api() -> Api {
    Api::new("t", ApiFormat::OpenApi31)
}

fn string(api: &mut Api, max: Option<u64>) -> TypeId {
    api.types.alloc_kind(TypeKind::String(StringType {
        max_length: max,
        ..StringType::default()
    }))
}

fn op(method: &str, path: &str) -> Operation {
    Operation::new(OperationKey::Http {
        method: HttpMethod::parse(method).unwrap(),
        path: PathTemplate::parse(path),
    })
}

fn param(name: &str, required: bool, ty: TypeId) -> Parameter {
    Parameter {
        location: ParamLocation::Query,
        name: name.into(),
        required,
        ty,
        meta: Meta::default(),
        origin: None,
    }
}

fn json(ty: TypeId) -> Vec<Content> {
    vec![Content {
        media_type: "application/json".into(),
        ty,
        origin: None,
    }]
}

fn response(status: &str, ty: TypeId) -> Response {
    Response {
        status: StatusKey::parse(status).unwrap(),
        headers: Vec::new(),
        content: json(ty),
        meta: Meta::default(),
        origin: None,
    }
}

fn codes(old: &Api, new: &Api) -> Vec<String> {
    diff(old, new)
        .iter()
        .map(|c| format!("{} {} [{}]", c.kind.code(), c.path, c.polarity))
        .collect()
}

#[test]
fn operations_parameters_and_responses() {
    let mut old = api();
    let s = string(&mut old, None);
    let mut get = op("get", "/pets/{id}");
    get.parameters.push(param("limit", false, s));
    get.responses.push(response("200", s));
    get.responses.push(response("404", s));
    old.operations.push(get);
    old.operations.push(op("delete", "/pets/{id}"));

    let mut new = api();
    let s = string(&mut new, None);
    let mut get = op("get", "/pets/{petId}");
    get.parameters.push(param("limit", true, s));
    get.parameters.push(param("sort", true, s));
    get.responses.push(response("200", s));
    new.operations.push(get);
    new.operations.push(op("post", "/pets"));

    assert_eq!(
        codes(&old, &new),
        [
            "operation-removed op[DELETE /pets/{}] [in]",
            "parameter-required-changed op[GET /pets/{}]/param[query:limit] [in]",
            "parameter-added op[GET /pets/{}]/param[query:sort] [in]",
            "response-removed op[GET /pets/{}]/response[404] [out]",
            "operation-added op[POST /pets] [in]",
        ]
    );
}

#[test]
fn bodies_and_media_types() {
    let mut old = api();
    let s = string(&mut old, Some(10));
    let mut post = op("post", "/pets");
    post.request_body = Some(RequestBody {
        required: false,
        content: json(s),
        meta: Meta::default(),
        origin: None,
    });
    post.responses.push(response("201", s));
    old.operations.push(post);

    let mut new = api();
    let s = string(&mut new, Some(5));
    let mut post = op("post", "/pets");
    let mut content = json(s);
    content.push(Content {
        media_type: "application/xml".into(),
        ty: s,
        origin: None,
    });
    post.request_body = Some(RequestBody {
        required: true,
        content,
        meta: Meta::default(),
        origin: None,
    });
    post.responses.push(response("201", s));
    new.operations.push(post);

    let set = diff(&old, &new);
    let summary: Vec<String> = set
        .iter()
        .map(|c| format!("{} {} [{}]", c.kind.code(), c.path, c.polarity))
        .collect();
    assert_eq!(
        summary,
        [
            "request-body-required-changed op[POST /pets] [in]",
            "bound-changed op[POST /pets]/request[application/json] [in]",
            "media-type-added op[POST /pets]/request[application/xml] [in]",
            "bound-changed op[POST /pets]/response[201]/body[application/json] [out]",
        ]
    );
    // The same narrowing loses values in the request and not in the response
    // direction — the diff records evidence; direction is for the rules.
    let request = &set.changes[1];
    assert!(request.effect.as_ref().unwrap().lost.witness().is_some());
}

#[test]
fn callbacks_reverse_polarity() {
    let build = |max: u64| {
        let mut a = api();
        let s = string(&mut a, Some(max));
        let mut notify = op("post", "{$request.body#/url}");
        notify.request_body = Some(RequestBody {
            required: true,
            content: json(s),
            meta: Meta::default(),
            origin: None,
        });
        let mut subscribe = op("post", "/subscriptions");
        subscribe.callbacks.push(Callback {
            name: "onEvent".into(),
            operations: vec![notify],
            origin: None,
        });
        a.operations.push(subscribe);
        a
    };
    let set = diff(&build(10), &build(5));
    assert_eq!(set.len(), 1);
    assert_eq!(
        set.changes[0].polarity,
        Polarity::OUT,
        "the API sends callback requests"
    );
    assert!(matches!(
        set.changes[0].kind,
        ChangeKind::BoundChanged { .. }
    ));
}
