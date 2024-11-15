//! Changes to the identity of fields and enum values, and to RPC shapes:
//! wire tags, aliases, exclusive groups, value numbers, encodings and
//! streaming. These come from formats such as Protobuf, so the APIs are
//! built directly in the IR.

use faultline_diff::{Change, ChangeKind, diff};
use faultline_ir::{
    Api, ApiFormat, Content, EnumType, EnumValue, ExclusiveGroup, Field, Literal, NumericType,
    ObjectType, Operation, OperationKey, RequestBody, Response, SchemaRoot, StatusKey, Streaming,
    StringType, TypeId, TypeKind,
};

/// A message field: `(name, tag, type)`.
type Spec<'a> = (&'a str, u32, Kind);

#[derive(Clone, Copy)]
enum Kind {
    Str,
    Int(&'static str, &'static str),
}

struct Message<'a> {
    fields: Vec<Spec<'a>>,
    groups: Vec<(&'a str, Vec<&'a str>)>,
    aliases: Vec<(&'a str, &'a str)>,
}

fn message<'a>(fields: &[Spec<'a>]) -> Message<'a> {
    Message {
        fields: fields.to_vec(),
        groups: Vec::new(),
        aliases: Vec::new(),
    }
}

fn scalar(api: &mut Api, kind: Kind) -> TypeId {
    api.types.alloc_kind(match kind {
        Kind::Str => TypeKind::String(StringType::default()),
        Kind::Int(format, encoding) => TypeKind::Integer(NumericType {
            format: Some(format.into()),
            encoding: Some(encoding.into()),
            ..NumericType::default()
        }),
    })
}

/// An API whose single schema root is `m`.
fn api(m: &Message<'_>) -> Api {
    let mut api = Api::new("test", ApiFormat::Protobuf);
    let any = api.types.alloc_kind(TypeKind::Any);
    let mut obj = ObjectType::new(any);
    for &(name, tag, kind) in &m.fields {
        let ty = scalar(&mut api, kind);
        let mut field = Field::new(name, ty, false);
        field.tag = Some(tag);
        field.aliases = m
            .aliases
            .iter()
            .filter(|(f, _)| *f == name)
            .map(|(_, a)| (*a).to_owned())
            .collect();
        obj.fields.push(field);
    }
    obj.exclusive = m
        .groups
        .iter()
        .map(|(name, fields)| ExclusiveGroup {
            name: (*name).to_owned(),
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            origin: None,
        })
        .collect();
    let root = api.types.alloc_kind(TypeKind::Object(obj));
    api.schemas.push(SchemaRoot {
        name: "m".into(),
        ty: root,
        role: faultline_ir::Role::Both,
        origin: None,
    });
    api
}

fn kinds(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|c| format!("{} {}", c.kind.code(), c.path))
        .collect()
}

#[test]
fn fields_are_matched_by_tag() {
    let old = api(&message(&[("name", 1, Kind::Str), ("id", 2, Kind::Str)]));
    let new = api(&message(&[("title", 1, Kind::Str), ("id", 2, Kind::Str)]));
    let changes = diff(&old, &new).changes;
    assert_eq!(kinds(&changes), ["field-renamed schema[m].title"]);
    assert_eq!(
        changes[0].kind,
        ChangeKind::FieldRenamed {
            old: "name".into(),
            new: "title".into(),
            tag: Some(1)
        }
    );
}

#[test]
fn a_name_on_another_tag_is_one_move() {
    let old = api(&message(&[("name", 1, Kind::Str)]));
    let new = api(&message(&[("name", 7, Kind::Str)]));
    let changes = diff(&old, &new).changes;
    assert_eq!(kinds(&changes), ["field-tag-changed schema[m].name"]);
    assert_eq!(
        changes[0].kind,
        ChangeKind::FieldTagChanged {
            name: "name".into(),
            old: 1,
            new: 7
        }
    );
}

#[test]
fn a_reused_tag_is_a_rename_with_a_type_change() {
    let old = api(&message(&[("name", 1, Kind::Str)]));
    let new = api(&message(&[("count", 1, Kind::Int("int32", "varint"))]));
    let codes = kinds(&diff(&old, &new).changes);
    assert!(
        codes.contains(&"field-renamed schema[m].count".to_owned()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"type-changed schema[m].count".to_owned()),
        "{codes:?}"
    );
}

#[test]
fn untagged_fields_are_matched_by_name() {
    let mut old = api(&message(&[("name", 1, Kind::Str)]));
    let mut new = api(&message(&[("title", 1, Kind::Str)]));
    for api in [&mut old, &mut new] {
        let root = api.schemas[0].ty;
        let TypeKind::Object(obj) = &mut api.types.get_mut(root).kind else {
            unreachable!()
        };
        obj.fields[0].tag = None;
    }
    let codes = kinds(&diff(&old, &new).changes);
    assert_eq!(
        codes,
        ["field-removed schema[m]", "field-added schema[m]"],
        "{codes:?}"
    );
}

#[test]
fn aliases_and_encodings() {
    let mut old = message(&[("user_id", 1, Kind::Int("int64", "varint"))]);
    old.aliases.push(("user_id", "userId"));
    let mut new = message(&[("user_id", 1, Kind::Int("sint64", "zigzag"))]);
    new.aliases.push(("user_id", "uid"));
    let changes = diff(&api(&old), &api(&new)).changes;
    let codes = kinds(&changes);
    assert!(
        codes.contains(&"field-aliases-changed schema[m].user_id".to_owned()),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"encoding-changed schema[m].user_id".to_owned()),
        "{codes:?}"
    );
    let aliases = changes
        .iter()
        .find(|c| c.kind.code() == "field-aliases-changed")
        .unwrap();
    assert_eq!(
        aliases.kind,
        ChangeKind::FieldAliasesChanged {
            name: "user_id".into(),
            removed: vec!["userId".into()],
            added: vec!["uid".into()],
        }
    );
    // Same encoding, different declared type: only the format differs.
    let old = api(&message(&[("n", 1, Kind::Int("int32", "varint"))]));
    let new = api(&message(&[("n", 1, Kind::Int("int64", "varint"))]));
    let codes = kinds(&diff(&old, &new).changes);
    assert!(
        !codes.iter().any(|c| c.starts_with("encoding-changed")),
        "{codes:?}"
    );
}

#[test]
fn moving_a_field_into_a_oneof_loses_combinations() {
    let fields = [("email", 1, Kind::Str), ("phone", 2, Kind::Str)];
    let old = message(&fields);
    let mut new = message(&fields);
    new.groups.push(("contact", vec!["email", "phone"]));
    let changes = diff(&api(&old), &api(&new)).changes;
    let moved: Vec<&Change> = changes
        .iter()
        .filter(|c| c.kind.code() == "field-group-changed")
        .collect();
    assert_eq!(moved.len(), 2, "{:?}", kinds(&changes));
    let effect = moved[0].effect.as_ref().unwrap();
    let witness = effect.lost.witness().expect("a verified witness");
    assert_eq!(
        witness.to_string(),
        r#"{"email":"","phone":""}"#,
        "an object setting both members"
    );
    assert!(effect.gained.is_none());

    // Renaming the group changes no values but is still reported.
    let mut renamed = message(&fields);
    renamed.groups.push(("reach", vec!["email", "phone"]));
    let changes = diff(&api(&new), &api(&renamed)).changes;
    assert_eq!(
        kinds(&changes),
        [
            "field-group-changed schema[m]",
            "field-group-changed schema[m]"
        ]
    );
    assert!(changes.iter().all(|c| {
        let e = c.effect.as_ref().unwrap();
        e.lost.is_none() && e.gained.is_none()
    }));
}

fn enum_api(values: &[(&str, i64)]) -> Api {
    let mut api = Api::new("test", ApiFormat::Protobuf);
    let values = values
        .iter()
        .map(|&(name, number)| {
            let mut v = EnumValue::new(Literal::string(name));
            v.name = Some(name.into());
            v.number = Some(number);
            v
        })
        .collect();
    let ty = api.types.alloc_kind(TypeKind::Enum(EnumType {
        values,
        open: true,
        // Numbered like Protobuf's, which can reserve numbers.
        retired: faultline_ir::Retired {
            declarable: true,
            ..faultline_ir::Retired::default()
        },
        writer_exact: false,
    }));
    api.schemas.push(SchemaRoot {
        name: "e".into(),
        ty,
        role: faultline_ir::Role::Both,
        origin: None,
    });
    api
}

#[test]
fn enum_values_are_identified_by_number_and_name() {
    let old = enum_api(&[("UNKNOWN", 0), ("CAT", 1), ("DOG", 2)]);
    let renamed = enum_api(&[("UNKNOWN", 0), ("FELINE", 1), ("DOG", 2)]);
    let changes = diff(&old, &renamed).changes;
    assert_eq!(kinds(&changes), ["enum-value-renamed schema[e]"]);
    assert_eq!(
        changes[0].kind,
        ChangeKind::EnumValueRenamed {
            number: 1,
            old: "CAT".into(),
            new: "FELINE".into()
        }
    );

    let renumbered = enum_api(&[("UNKNOWN", 0), ("CAT", 3), ("DOG", 2)]);
    let changes = diff(&old, &renumbered).changes;
    // Number 1 is now free, and not reserved.
    assert_eq!(
        kinds(&changes),
        [
            "enum-value-renumbered schema[e]",
            "number-not-retired schema[e]"
        ]
    );

    let added = enum_api(&[("UNKNOWN", 0), ("CAT", 1), ("DOG", 2), ("FISH", 3)]);
    assert_eq!(
        kinds(&diff(&old, &added).changes),
        ["enum-value-added schema[e]"]
    );
}

fn rpc(streaming: Streaming) -> Api {
    let mut api = Api::new("test", ApiFormat::Protobuf);
    let ty = api
        .types
        .alloc_kind(TypeKind::String(StringType::default()));
    let mut op = Operation::new(OperationKey::Rpc {
        service: "pets.v1.PetService".into(),
        method: "Watch".into(),
    });
    op.request_body = Some(RequestBody {
        required: true,
        content: vec![Content {
            media_type: "application/grpc".into(),
            ty,
            origin: None,
        }],
        meta: faultline_ir::Meta::default(),
        origin: None,
    });
    op.responses.push(Response {
        status: StatusKey::Default,
        headers: Vec::new(),
        content: vec![Content {
            media_type: "application/grpc".into(),
            ty,
            origin: None,
        }],
        meta: faultline_ir::Meta::default(),
        origin: None,
    });
    op.streaming = Some(streaming);
    api.operations.push(op);
    api
}

#[test]
fn streaming_changes_per_direction() {
    let unary = rpc(Streaming::default());
    let server = rpc(Streaming {
        request: false,
        response: true,
    });
    let changes = diff(&unary, &server).changes;
    assert_eq!(
        kinds(&changes),
        ["streaming-changed op[pets.v1.PetService/Watch]"]
    );
    assert_eq!(
        changes[0].kind,
        ChangeKind::StreamingChanged {
            side: "response".into(),
            streaming: true
        }
    );
    assert!(changes[0].polarity.is_out());
    let both = rpc(Streaming {
        request: true,
        response: true,
    });
    assert_eq!(diff(&server, &both).len(), 1);
    assert!(diff(&both, &both).is_empty());
}

#[test]
fn numbers_are_only_retired_where_the_format_can() {
    let old = enum_api(&[("UNKNOWN", 0), ("CAT", 1), ("DOG", 2)]);
    let mut new = enum_api(&[("UNKNOWN", 0), ("DOG", 2)]);
    for id in new.types.ids().collect::<Vec<_>>() {
        if let TypeKind::Enum(e) = &mut new.types.get_mut(id).kind {
            e.retired.declarable = false;
        }
    }
    let changes = diff(&old, &new).changes;
    assert!(
        !kinds(&changes)
            .iter()
            .any(|k| k.starts_with("number-not-retired")),
        "{:?}",
        kinds(&changes)
    );
}

#[test]
fn collection_encodings_are_compared() {
    let api = |encoding: &str| {
        let mut api = Api::new("t", ApiFormat::Thrift);
        let item = api.types.alloc_kind(TypeKind::Boolean);
        let ty = api
            .types
            .alloc_kind(TypeKind::Array(faultline_ir::ArrayType {
                encoding: Some(encoding.into()),
                ..faultline_ir::ArrayType::of(item)
            }));
        api.schemas.push(SchemaRoot {
            name: "a".into(),
            ty,
            role: faultline_ir::Role::Both,
            origin: None,
        });
        api
    };
    let changes = diff(&api("list"), &api("set")).changes;
    assert_eq!(kinds(&changes), ["encoding-changed schema[a]"]);
    assert!(diff(&api("list"), &api("list")).changes.is_empty());
}
