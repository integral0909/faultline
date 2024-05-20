mod common;

use common::{Builder, callback, http, param, request, response};
use faultline_ir::{Access, ParamLocation, Polarities, Polarity, Role, SchemaRoot, TypeKind};

#[test]
fn request_response_and_shared_types() {
    let mut b = Builder::new();
    let query = b.string();
    let created = b.string();
    let shared = b.string();
    let input = b.object(&[("name", shared, true)]);
    let output = b.object(&[("name", shared, true), ("created", created, true)]);

    let mut op = http("post", "/pets");
    op.parameters
        .push(param(ParamLocation::Query, "dryRun", query));
    op.request_body = Some(request(input));
    op.responses.push(response("201", output));
    b.api.operations.push(op);

    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(query), Polarity::IN);
    assert_eq!(p.get(input), Polarity::IN);
    assert_eq!(p.get(output), Polarity::OUT);
    assert_eq!(p.get(created), Polarity::OUT);
    assert_eq!(p.get(shared), Polarity::BOTH);
}

#[test]
fn read_only_and_write_only_restrict_their_subtrees() {
    let mut b = Builder::new();
    let id_inner = b.string();
    let id = b.object(&[("value", id_inner, true)]);
    b.with_access(id, Access::ReadOnly);
    let password = b.string();
    b.with_access(password, Access::WriteOnly);
    let name = b.string();
    let pet = b.named(
        "Pet",
        TypeKind::Any, // replaced below once the fields exist
    );
    let body = b.object(&[
        ("id", id, false),
        ("password", password, false),
        ("name", name, true),
    ]);
    b.api.types.get_mut(pet).kind = TypeKind::Ref(body);

    let mut op = http("put", "/pets/{id}");
    op.request_body = Some(request(pet));
    op.responses.push(response("200", pet));
    b.api.operations.push(op);

    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(pet), Polarity::BOTH);
    assert_eq!(p.get(name), Polarity::BOTH);
    assert_eq!(p.get(id), Polarity::OUT);
    assert_eq!(
        p.get(id_inner),
        Polarity::OUT,
        "restriction applies to the whole subtree"
    );
    assert_eq!(p.get(password), Polarity::IN);
}

#[test]
fn a_read_only_type_used_only_in_requests_is_unreachable() {
    let mut b = Builder::new();
    let ro = b.string();
    b.with_access(ro, Access::ReadOnly);
    let mut op = http("post", "/x");
    op.request_body = Some(request(ro));
    b.api.operations.push(op);
    assert_eq!(Polarities::compute(&b.api).get(ro), Polarity::NONE);
}

#[test]
fn callbacks_invert_direction_and_nesting_inverts_back() {
    let mut b = Builder::new();
    let event = b.string();
    let ack = b.string();
    let nested_request = b.string();

    let mut nested = http("post", "{$request.body#/ack}");
    nested.request_body = Some(request(nested_request));

    let mut notify = http("post", "{$request.body#/callbackUrl}");
    notify.request_body = Some(request(event));
    notify.responses.push(response("200", ack));
    notify.callbacks.push(callback("confirm", vec![nested]));

    let subscribe_body = b.string();
    let mut subscribe = http("post", "/subscriptions");
    subscribe.request_body = Some(request(subscribe_body));
    subscribe.callbacks.push(callback("onEvent", vec![notify]));
    b.api.operations.push(subscribe);

    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(subscribe_body), Polarity::IN);
    assert_eq!(
        p.get(event),
        Polarity::OUT,
        "the API sends callback requests"
    );
    assert_eq!(
        p.get(ack),
        Polarity::IN,
        "the API receives callback responses"
    );
    assert_eq!(p.get(nested_request), Polarity::IN);

    let paths: Vec<String> = b.api.roots().iter().map(|r| r.path.to_string()).collect();
    assert_eq!(
        paths,
        [
            "op[POST /subscriptions]/request[application/json]",
            "op[POST /subscriptions]/callback[onEvent]/op[POST {}]/request[application/json]",
            "op[POST /subscriptions]/callback[onEvent]/op[POST {}]/response[200]/body[application/json]",
            "op[POST /subscriptions]/callback[onEvent]/op[POST {}]/callback[confirm]/op[POST {}]/request[application/json]",
        ]
    );
}

#[test]
fn negation_inverts_direction_for_its_operand() {
    let mut b = Builder::new();
    let operand = b.string();
    let negated = b.kind(TypeKind::Not(operand));
    let mut op = http("post", "/x");
    op.request_body = Some(request(negated));
    b.api.operations.push(op);

    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(negated), Polarity::IN);
    assert_eq!(p.get(operand), Polarity::OUT);
}

#[test]
fn cyclic_types_terminate_and_are_fully_marked() {
    let mut b = Builder::new();
    let tree = b.reserve_named("Tree");
    let child_ref = b.reference(tree);
    let children = b.array(child_ref);
    let label = b.string();
    let additional = b.any();
    let mut obj = faultline_ir::ObjectType::new(additional);
    obj.fields = vec![
        faultline_ir::Field::new("label", label, true),
        faultline_ir::Field::new("children", children, false),
    ];
    b.define_named(tree, "Tree", TypeKind::Object(obj));

    let mut op = http("get", "/tree");
    op.responses.push(response("200", tree));
    b.api.operations.push(op);

    let p = Polarities::compute(&b.api);
    for id in [tree, child_ref, children, label, additional] {
        assert_eq!(p.get(id), Polarity::OUT, "{id:?}");
    }
    assert!(faultline_ir::validate(&b.api).is_empty());
}

#[test]
fn schema_roots_use_their_role() {
    let mut b = Builder::new();
    let config = b.string();
    let event = b.string();
    let both = b.string();
    for (name, ty, role) in [
        ("config", config, Role::Input),
        ("event", event, Role::Output),
        ("record", both, Role::Both),
    ] {
        b.api.schemas.push(SchemaRoot {
            name: name.into(),
            ty,
            role,
            origin: None,
        });
    }
    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(config), Polarity::IN);
    assert_eq!(p.get(event), Polarity::OUT);
    assert_eq!(p.get(both), Polarity::BOTH);
}

#[test]
fn unreachable_types_have_no_polarity() {
    let mut b = Builder::new();
    let orphan = b.named("Orphan", TypeKind::Boolean);
    assert_eq!(Polarities::compute(&b.api).get(orphan), Polarity::NONE);
}

#[test]
fn field_arguments_flow_against_the_field() {
    let mut b = Builder::new();
    let first = b.string();
    let arguments = b.object(&[("first", first, false)]);
    let item = b.string();
    let list = b.array(item);
    let user = b.object(&[("pets", list, true)]);
    let TypeKind::Object(obj) = &mut b.api.types.get_mut(user).kind else {
        unreachable!()
    };
    obj.fields[0].arguments = Some(arguments);

    let mut op = http("get", "/me");
    op.responses.push(response("200", user));
    b.api.operations.push(op);

    let p = Polarities::compute(&b.api);
    assert_eq!(p.get(user), Polarity::OUT);
    assert_eq!(p.get(item), Polarity::OUT);
    assert_eq!(p.get(arguments), Polarity::IN, "the reader sends arguments");
    assert_eq!(p.get(first), Polarity::IN);
}
