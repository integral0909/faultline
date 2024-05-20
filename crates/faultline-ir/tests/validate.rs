mod common;

use common::{Builder, http, param, response};
use faultline_doc::Decimal;
use faultline_ir::{
    EnumType, EnumValue, IrError, Literal, ParamLocation, QualName, TypeId, TypeKind, TypeNode,
    UnionMode, UnionType, validate,
};

fn number(text: &str) -> Literal {
    Literal::Number(Decimal::parse(text).unwrap().into())
}

#[test]
fn a_well_formed_api_has_no_errors() {
    let mut b = Builder::new();
    let name = b.string();
    let pet = b.object(&[("name", name, true)]);
    let mut op = http("get", "/pets/{id}");
    op.responses.push(response("200", pet));
    b.api.operations.push(op);
    assert_eq!(validate(&b.api), []);
}

#[test]
fn undefined_and_dangling_types() {
    let mut b = Builder::new();
    let reserved = b.api.types.reserve();
    let dangling = b.kind(TypeKind::Ref(TypeId::from_raw(99)));
    assert_eq!(
        validate(&b.api),
        [
            IrError::UndefinedType { id: reserved },
            IrError::DanglingReference {
                from: Some(dangling),
                to: TypeId::from_raw(99)
            },
        ]
    );
}

#[test]
fn unguarded_cycles_are_rejected() {
    // A = allOf[A]
    let mut b = Builder::new();
    let a = b.api.types.reserve();
    b.api
        .types
        .define(a, TypeNode::new(TypeKind::AllOf(vec![a])));
    assert_eq!(
        validate(&b.api),
        [IrError::UnguardedCycle { cycle: vec![a] }]
    );

    // A = ref B, B = oneOf[string, ref A]
    let mut b = Builder::new();
    let a = b.api.types.reserve();
    let s = b.string();
    let back = b.reference(a);
    let union = b.kind(TypeKind::Union(UnionType {
        mode: UnionMode::Exclusive,
        variants: vec![s, back],
        discriminator: None,
    }));
    b.api.types.define(a, TypeNode::new(TypeKind::Ref(union)));
    let errors = validate(&b.api);
    assert_eq!(errors.len(), 1, "{errors:?}");
    let IrError::UnguardedCycle { cycle } = &errors[0] else {
        panic!("{errors:?}")
    };
    let mut sorted = cycle.clone();
    sorted.sort();
    assert_eq!(sorted, [a, back, union]);
}

#[test]
fn recursion_through_fields_and_items_is_fine() {
    let mut b = Builder::new();
    let list = b.api.types.reserve();
    let next = b.reference(list);
    let items = b.array(next);
    let obj = b.object(&[("items", items, false)]);
    b.api.types.define(list, TypeNode::new(TypeKind::Ref(obj)));
    assert_eq!(validate(&b.api), []);
}

#[test]
fn duplicate_fields_and_semantically_equal_enum_values() {
    let mut b = Builder::new();
    let s = b.string();
    let obj = b.object(&[("a", s, true), ("b", s, false), ("a", s, false)]);
    let e = b.kind(TypeKind::Enum(EnumType {
        values: vec![
            EnumValue::new(number("1")),
            EnumValue::new(number("1.0")),
            EnumValue::new(Literal::string("1")),
        ],
        open: false,
        retired: faultline_ir::Retired::default(),
    }));
    assert_eq!(
        validate(&b.api),
        [
            IrError::DuplicateField {
                ty: obj,
                name: "a".into()
            },
            IrError::DuplicateEnumValue {
                ty: e,
                value: number("1")
            },
        ]
    );
}

#[test]
fn operations_differing_only_in_parameter_names_collide() {
    let mut b = Builder::new();
    b.api.operations.push(http("get", "/pets/{id}"));
    b.api.operations.push(http("get", "/pets/{petId}"));
    b.api.operations.push(http("put", "/pets/{petId}"));
    assert_eq!(
        validate(&b.api),
        [IrError::DuplicateOperation {
            identity: "GET /pets/{}".into()
        }]
    );
}

#[test]
fn header_parameters_differing_only_in_case_collide() {
    let mut b = Builder::new();
    let s = b.string();
    let mut op = http("get", "/x");
    op.parameters
        .push(param(ParamLocation::Header, "X-Trace", s));
    op.parameters
        .push(param(ParamLocation::Header, "x-trace", s));
    op.parameters
        .push(param(ParamLocation::Query, "x-trace", s));
    b.api.operations.push(op);
    let errors = validate(&b.api);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].to_string(),
        "root op[GET /x]/param[header:x-trace] is declared twice"
    );
}

#[test]
fn named_map_must_agree_with_node_names() {
    let mut b = Builder::new();
    let pet = b.named("Pet", TypeKind::Boolean);
    b.api.named.insert(QualName::new("Alias"), pet);
    assert_eq!(
        validate(&b.api),
        [IrError::NameMismatch {
            name: QualName::new("Alias"),
            ty: pet
        }]
    );
}

#[test]
fn exclusive_groups_must_name_declared_fields_once() {
    let mut b = Builder::new();
    let s = b.string();
    let id = b.object(&[("email", s, false), ("phone", s, false)]);
    let TypeKind::Object(obj) = &mut b.api.types.get_mut(id).kind else {
        unreachable!()
    };
    let group = |name: &str, fields: &[&str]| faultline_ir::ExclusiveGroup {
        name: name.into(),
        fields: fields.iter().map(|f| (*f).to_owned()).collect(),
        origin: None,
    };
    obj.exclusive = vec![
        group("contact", &["email", "phone"]),
        group("other", &["phone", "fax"]),
    ];
    assert_eq!(
        validate(&b.api),
        [
            IrError::InvalidExclusiveGroup {
                ty: id,
                group: "other".into(),
                field: "phone".into()
            },
            IrError::InvalidExclusiveGroup {
                ty: id,
                group: "other".into(),
                field: "fax".into()
            },
        ]
    );
}
