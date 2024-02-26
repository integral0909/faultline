mod common;

use common::{Builder, http, param, request, response};
use faultline_doc::Decimal;
use faultline_ir::{
    Access, Bound, EnumType, EnumValue, Literal, NumericType, ParamLocation, StringType, TypeKind,
    UnionMode, UnionType, dump,
};

/// Builds a small pet store. `reverse` allocates the independent types in
/// the opposite order, which must not change the dump.
fn petstore(reverse: bool) -> faultline_ir::Api {
    let mut b = Builder::new();
    b.api.name = "petstore".into();
    b.api.version = Some("1.0.0".into());

    let make_limit = |b: &mut Builder| {
        b.kind(TypeKind::Integer(NumericType {
            minimum: Some(Bound::inclusive(Decimal::parse("1").unwrap())),
            maximum: Some(Bound::exclusive(Decimal::parse("100").unwrap())),
            multiple_of: None,
            format: Some("int32".into()),
        }))
    };
    let make_status = |b: &mut Builder| {
        b.kind(TypeKind::Enum(EnumType {
            values: ["available", "sold"]
                .into_iter()
                .map(|s| EnumValue::new(Literal::string(s)))
                .collect(),
            open: false,
        }))
    };
    let (limit, status) = if reverse {
        let s = make_status(&mut b);
        (make_limit(&mut b), s)
    } else {
        let l = make_limit(&mut b);
        (l, make_status(&mut b))
    };

    let pet = b.reserve_named("Pet");
    let id = b.kind(TypeKind::Integer(NumericType::default()));
    b.with_access(id, Access::ReadOnly);
    let name = b.kind(TypeKind::String(StringType {
        min_length: 1,
        max_length: Some(64),
        pattern: None,
        format: None,
    }));
    let body = b.object(&[
        ("id", id, false),
        ("name", name, true),
        ("status", status, false),
    ]);
    b.define_named(pet, "Pet", TypeKind::Ref(body));
    let pet_ref = b.reference(pet);
    let pets = b.array(pet_ref);
    let null = b.kind(TypeKind::Null);
    let maybe_pet = b.kind(TypeKind::Union(UnionType {
        mode: UnionMode::Any,
        variants: vec![pet, null],
        discriminator: None,
    }));

    let mut list = http("get", "/pets");
    list.parameters
        .push(param(ParamLocation::Query, "limit", limit));
    list.responses.push(response("200", pets));
    let mut create = http("post", "/pets");
    create.request_body = Some(request(pet));
    create.responses.push(response("201", maybe_pet));
    b.api.operations.push(list);
    b.api.operations.push(create);

    b.named("Unused", TypeKind::Boolean);
    b.api
}

#[test]
fn petstore_dump() {
    let actual = dump(&petstore(false));
    let expected = "\
api petstore (openapi-3.1) version 1.0.0

op[GET /pets]/param[query:limit] [in] -> t1
op[GET /pets]/response[200]/body[application/json] [out] -> t2
op[POST /pets]/request[application/json] [in] -> Pet
op[POST /pets]/response[201]/body[application/json] [out] -> t9

t1 [in]: integer format=int32 min=1 exclusiveMax=100
t2 [out]: array items=t3
t3 [out]: ref Pet
Pet [both]: ref t4
t4 [both]: object additional=t8
  .id: t5
  .name required: t6
  .status: t7
t5 [out]: integer readOnly
t6 [both]: string minLength=1 maxLength=64
t7 [both]: enum
  = \"available\"
  = \"sold\"
t8 [both]: any
t9 [out]: union any [Pet, t10]
t10 [out]: null
Unused [none]: boolean
";
    assert_eq!(actual, expected, "\n{actual}");
}

#[test]
fn dump_does_not_depend_on_allocation_order() {
    assert_eq!(dump(&petstore(false)), dump(&petstore(true)));
}

#[test]
fn dumping_a_deep_chain_does_not_overflow_the_stack() {
    let mut b = Builder::new();
    let mut ty = b.string();
    for _ in 0..200_000 {
        ty = b.array(ty);
    }
    b.api.schemas.push(faultline_ir::SchemaRoot {
        name: "deep".into(),
        ty,
        role: faultline_ir::Role::Input,
        origin: None,
    });
    let text = dump(&b.api);
    assert!(text.ends_with("string\n"));
    assert!(faultline_ir::validate(&b.api).is_empty());
}
