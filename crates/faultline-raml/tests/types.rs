//! RAML data types: facets, properties, inheritance, unions and changes.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ApiFormat, Role, SchemaRoot, TypeKind, validate};
use faultline_raml::RamlLoader;
use faultline_raml::types::{Scope, TypeLowerer};
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceId, SourceMap, Span};

/// The declared type `name` of a RAML document with `types`, as the root
/// of an API used in `role`, and the lowering's diagnostic codes.
fn lowered(types: &str, name: &str, role: Role) -> (Api, Vec<String>) {
    let text = format!("#%RAML 1.0\ntitle: T\ntypes:\n{types}");
    let loader = MemoryLoader::new().with("file:///api.raml", text);
    let sources = SourceMap::new();
    let mut raml = RamlLoader::new(&loader, &sources, Limits::default());
    let document = raml
        .document(&UriRef::parse("file:///api.raml").unwrap(), None)
        .unwrap_or_else(|| panic!("{:?}", raml.diagnostics));
    let scope = Scope::of(&document.root, "");
    let mut api = Api::new("t", ApiFormat::JsonSchema);
    let (ty, codes) = {
        let mut lowerer = TypeLowerer::new(&mut api);
        let ty = lowerer.named(name, Span::empty(SourceId::from_raw(0), 0), &scope);
        let codes = lowerer
            .diagnostics
            .iter()
            .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
            .collect();
        (ty, codes)
    };
    api.schemas.push(SchemaRoot {
        name: name.to_owned(),
        ty,
        role,
        origin: None,
    });
    (api, codes)
}

fn kind(types: &str, name: &str) -> (Api, TypeKind) {
    let (api, codes) = lowered(types, name, Role::Both);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(validate(&api), []);
    let id = api.types.resolve(api.schemas[0].ty);
    let kind = api.types.get(id).kind.clone();
    (api, kind)
}

fn worst(old: &str, new: &str, role: Role) -> Severity {
    let (a, _) = lowered(old, "T", role);
    let (b, _) = lowered(new, "T", role);
    faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| classify("t", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

const PET: &str = "  Named:
    properties:
      name:
        type: string
        maxLength: 64
  Pet:
    type: Named
    properties:
      tag?: string
      age:
        type: integer
        format: int8
        required: false
      owner: Person | nil
      /^x-/: string
    additionalProperties: false
  Person:
    properties:
      name: string
      friends: Person[]
";

#[test]
fn objects_inherit_and_declare_properties() {
    let (api, TypeKind::Object(pet)) = kind(PET, "Pet") else {
        panic!()
    };
    let fields: Vec<(&str, bool)> = pet
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        fields,
        [
            ("name", true),
            ("tag", false),
            ("age", false),
            ("owner", true)
        ],
        "inherited first; `?` and `required: false` make properties optional"
    );
    assert_eq!(pet.pattern_properties[0].0, "^x-");
    assert!(matches!(
        api.types.get(pet.additional).kind,
        TypeKind::Never
    ));
    let TypeKind::Integer(age) = &api.types.get(pet.field("age").unwrap().ty).kind else {
        panic!()
    };
    assert_eq!(
        age.maximum.as_ref().unwrap().value.to_string(),
        "127",
        "int8"
    );
    let owner = api.types.get(pet.field("owner").unwrap().ty);
    assert!(matches!(&owner.kind, TypeKind::Union(u) if u.variants.len() == 2));
    // A recursive type refers to itself.
    assert!(api.named_type("Person").is_some());
}

#[test]
fn scalars_and_arrays() {
    let types = "  Code:
    type: string
    pattern: ^[A-Z]{3}$
    minLength: 3
  Codes:
    type: Code[]
    maxItems: 5
  Size:
    enum: [S, M, L]
  Shape:
    properties:
      id: integer
    minProperties: 1
";
    let (_, TypeKind::String(code)) = kind(types, "Code") else {
        panic!()
    };
    assert_eq!(code.pattern.as_deref(), Some("^[A-Z]{3}$"));
    assert_eq!(code.min_length, 3);
    let (_, TypeKind::Array(codes)) = kind(types, "Codes") else {
        panic!()
    };
    assert_eq!(codes.max_items, Some(5));
    let (_, TypeKind::Enum(size)) = kind(types, "Size") else {
        panic!()
    };
    assert_eq!(size.values.len(), 3);
    let (_, TypeKind::Object(shape)) = kind(types, "Shape") else {
        panic!()
    };
    assert_eq!(shape.min_properties, 1);
}

#[test]
fn changes_have_directions() {
    let with = |extra: &str| format!("  T:\n    properties:\n      a: string\n{extra}");
    // A new required property breaks writers.
    assert_eq!(
        worst(&with(""), &with("      b: string\n"), Role::Input),
        Severity::Breaking
    );
    // An optional one does not.
    assert!(worst(&with(""), &with("      b?: string\n"), Role::Input) < Severity::Breaking);
    let limit = |n: u32| format!("  T:\n    type: string\n    maxLength: {n}\n");
    assert_eq!(
        worst(&limit(10), &limit(5), Role::Input),
        Severity::Breaking
    );
    assert!(worst(&limit(10), &limit(20), Role::Input) < Severity::Breaking);
}

#[test]
fn problems_are_reported() {
    assert_eq!(lowered("  T: Missing\n", "T", Role::Both).1, ["FLE7605"]);
    assert_eq!(lowered("  T: Pet[\n", "T", Role::Both).1, ["FLE7606"]);
    assert_eq!(
        lowered(
            "  T:\n    type: string\n    maxLength: -1\n",
            "T",
            Role::Both
        )
        .1,
        ["FLE7604"]
    );
    assert_eq!(
        lowered("  T:\n    type: T\n", "T", Role::Both).1,
        ["FLE7604"]
    );
}

const PETS: &str = "  Pet:
    discriminator: kind
    properties:
      kind: string
      name: string
  Cat:
    type: Pet
    properties:
      indoor: boolean
  Dog:
    type: Pet
    discriminatorValue: dog
  T: Cat | Dog
";

fn discriminator_values(api: &Api, ty: faultline_ir::TypeId) -> Vec<String> {
    let TypeKind::Object(object) = &api.types.get(api.types.resolve(ty)).kind else {
        panic!()
    };
    let kind = object.field("kind").unwrap();
    assert!(kind.required);
    let TypeKind::Enum(values) = &api.types.get(kind.ty).kind else {
        panic!()
    };
    values.values.iter().map(|v| v.value.to_string()).collect()
}

#[test]
fn discriminators_fix_their_property() {
    let (api, kind) = kind(PETS, "T");
    let TypeKind::Union(union) = kind else {
        panic!("{kind:?}")
    };
    let discriminator = union.discriminator.as_ref().unwrap();
    assert_eq!(discriminator.property, "kind");
    let values: Vec<&str> = discriminator
        .mapping
        .iter()
        .map(|(v, _)| v.as_str())
        .collect();
    assert_eq!(values, ["Cat", "dog"], "the type's name unless given");
    assert_eq!(discriminator_values(&api, union.variants[0]), ["\"Cat\""]);
    assert_eq!(discriminator_values(&api, union.variants[1]), ["\"dog\""]);
    let pet = api.named[&faultline_ir::QualName::new("Pet")];
    assert_eq!(discriminator_values(&api, pet), ["\"Pet\""]);
}

#[test]
fn discriminator_changes_are_found() {
    let renamed = PETS.replace("discriminatorValue: dog", "discriminatorValue: hound");
    assert_eq!(
        worst(PETS, &renamed, Role::Input),
        Severity::Breaking,
        "requests with the old value are rejected"
    );
    assert_eq!(
        worst(PETS, &renamed, Role::Output),
        Severity::Risky,
        "clients may not know the new value"
    );
    let fewer = PETS.replace("T: Cat | Dog", "T: Cat");
    assert_eq!(worst(PETS, &fewer, Role::Input), Severity::Breaking);
}

#[test]
fn discriminators_name_a_property() {
    let (_, codes) = lowered(
        "  T:\n    discriminator: kind\n    properties:\n      name: string\n",
        "T",
        Role::Both,
    );
    assert_eq!(codes, ["FLE7604"]);
}
