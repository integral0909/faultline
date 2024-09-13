//! Simple types: built-in datatypes, facets, lists and unions.

use std::fmt::Write as _;

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{Decimal, UriRef};
use faultline_ir::{Api, Role, SchemaRoot, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceMap, Span};
use faultline_xsd::model::QName;
use faultline_xsd::{Lowerer, Reader};

fn schema(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t" targetNamespace="urn:t">{body}</xs:schema>"#
    )
}

/// The schema's simple type `name`, as the only root of an API used in
/// `role`, and the lowering's diagnostics.
fn lowered(body: &str, name: &str, role: Role) -> (Api, Vec<String>) {
    let loader = MemoryLoader::new().with("file:///t.xsd", schema(body));
    let sources = SourceMap::new();
    let mut reader = Reader::new(&loader, &sources, Limits::default());
    reader.load(&UriRef::parse("file:///t.xsd").unwrap());
    assert!(reader.diagnostics.is_empty(), "{:?}", reader.diagnostics);
    let mut lowerer = Lowerer::new(&reader.components, "t");
    let span = Span::empty(faultline_source::SourceId::from_raw(0), 0);
    let ty = lowerer.simple_named(&QName::new(Some("urn:t"), name), span);
    lowerer.api.schemas.push(SchemaRoot {
        name: name.to_owned(),
        ty,
        role,
        origin: None,
    });
    let codes = lowerer
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowerer.api, codes)
}

fn kind(body: &str, name: &str) -> TypeKind {
    let (api, codes) = lowered(body, name, Role::Both);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(validate(&api), []);
    let id = api.types.resolve(api.schemas[0].ty);
    api.types.get(id).kind.clone()
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

#[test]
fn facets_narrow_their_base() {
    let TypeKind::String(s) = kind(
        r#"<xs:simpleType name="Code"><xs:restriction base="xs:token">
             <xs:minLength value="2"/><xs:maxLength value="8"/><xs:pattern value="[A-Z]+"/>
           </xs:restriction></xs:simpleType>"#,
        "Code",
    ) else {
        panic!()
    };
    assert_eq!((s.min_length, s.max_length), (2, Some(8)));
    assert_eq!(
        s.pattern.as_deref(),
        Some("^(?:[A-Z]+)$"),
        "patterns are anchored"
    );

    let TypeKind::Integer(n) = kind(
        r#"<xs:simpleType name="Percent"><xs:restriction base="xs:int">
             <xs:minInclusive value="0"/><xs:maxExclusive value="101"/>
           </xs:restriction></xs:simpleType>"#,
        "Percent",
    ) else {
        panic!()
    };
    assert_eq!(n.minimum.unwrap().value, Decimal::from_i128(0));
    let max = n.maximum.unwrap();
    assert_eq!((max.value, max.exclusive), (Decimal::from_i128(101), true));

    let TypeKind::Integer(_) = kind(
        r#"<xs:simpleType name="Whole"><xs:restriction base="xs:decimal"><xs:fractionDigits value="0"/></xs:restriction></xs:simpleType>"#,
        "Whole",
    ) else {
        panic!("no fraction digits make a decimal an integer")
    };
    let TypeKind::Number(money) = kind(
        r#"<xs:simpleType name="Money"><xs:restriction base="xs:decimal"><xs:totalDigits value="5"/><xs:fractionDigits value="2"/></xs:restriction></xs:simpleType>"#,
        "Money",
    ) else {
        panic!()
    };
    assert_eq!(money.maximum.unwrap().value, Decimal::from_i128(100_000));
    assert_eq!(money.multiple_of.unwrap().to_string(), "0.01");
}

#[test]
fn restrictions_of_restrictions_accumulate() {
    let body = r#"
      <xs:simpleType name="Short"><xs:restriction base="xs:string"><xs:maxLength value="10"/><xs:pattern value="[a-z]*"/></xs:restriction></xs:simpleType>
      <xs:simpleType name="Shorter"><xs:restriction base="t:Short"><xs:maxLength value="20"/><xs:minLength value="1"/><xs:pattern value="a.*"/></xs:restriction></xs:simpleType>"#;
    let TypeKind::AllOf(parts) = kind(body, "Shorter") else {
        panic!("two patterns both apply")
    };
    assert_eq!(parts.len(), 2);
    let (api, _) = lowered(body, "Shorter", Role::Both);
    let TypeKind::String(first) = &api.types.get(parts[0]).kind else {
        panic!()
    };
    assert_eq!(
        (first.min_length, first.max_length),
        (1, Some(10)),
        "a restriction cannot widen"
    );
}

#[test]
fn enumerations_lists_and_unions() {
    let TypeKind::Enum(e) = kind(
        r#"<xs:simpleType name="Size"><xs:restriction base="xs:int"><xs:enumeration value="1"/><xs:enumeration value="2"/></xs:restriction></xs:simpleType>"#,
        "Size",
    ) else {
        panic!()
    };
    let values: Vec<String> = e.values.iter().map(|v| v.value.to_string()).collect();
    assert_eq!(values, ["1", "2"], "numeric enumerations hold numbers");
    let TypeKind::Array(list) = kind(
        r#"<xs:simpleType name="Tags"><xs:restriction><xs:simpleType><xs:list itemType="xs:NCName"/></xs:simpleType><xs:maxLength value="3"/></xs:restriction></xs:simpleType>"#,
        "Tags",
    ) else {
        panic!()
    };
    assert_eq!(list.max_items, Some(3), "list lengths count items");
    let TypeKind::Union(u) = kind(
        r#"<xs:simpleType name="Id"><xs:union memberTypes="xs:int"><xs:simpleType><xs:restriction base="xs:string"/></xs:simpleType></xs:union></xs:simpleType>"#,
        "Id",
    ) else {
        panic!()
    };
    assert_eq!(u.variants.len(), 2);
}

#[test]
fn changes_have_directions() {
    let size = |values: &str| {
        let items = values.split(',').fold(String::new(), |mut out, v| {
            let _ = write!(out, r#"<xs:enumeration value="{v}"/>"#);
            out
        });
        format!(
            r#"<xs:simpleType name="T"><xs:restriction base="xs:string">{items}</xs:restriction></xs:simpleType>"#
        )
    };
    assert_eq!(
        worst(&size("S,M,L"), &size("S,M"), Role::Input),
        Severity::Breaking
    );
    assert!(worst(&size("S,M,L"), &size("S,M"), Role::Output) < Severity::Breaking);
    let int = |ty: &str| {
        format!(r#"<xs:simpleType name="T"><xs:restriction base="xs:{ty}"/></xs:simpleType>"#)
    };
    assert_eq!(
        worst(&int("int"), &int("short"), Role::Input),
        Severity::Breaking
    );
    assert!(worst(&int("int"), &int("long"), Role::Input) < Severity::Breaking);
    assert_eq!(
        worst(&int("int"), &int("string"), Role::Input),
        Severity::Breaking
    );
}

#[test]
fn problems_are_reported() {
    let (_, codes) = lowered(
        r#"<xs:simpleType name="A"><xs:restriction base="t:Missing"/></xs:simpleType>"#,
        "A",
        Role::Both,
    );
    assert_eq!(codes, ["FLE7204"]);
    let (_, codes) = lowered(
        r#"<xs:simpleType name="A"><xs:restriction base="t:B"/></xs:simpleType><xs:simpleType name="B"><xs:restriction base="t:A"/></xs:simpleType>"#,
        "A",
        Role::Both,
    );
    assert_eq!(codes, ["FLE7206"]);
    let (_, codes) = lowered(
        r#"<xs:simpleType name="A"><xs:restriction base="xs:int"><xs:maxInclusive value="ten"/></xs:restriction></xs:simpleType>"#,
        "A",
        Role::Both,
    );
    assert_eq!(codes, ["FLE7206"]);
    let (_, codes) = lowered(
        r#"<xs:simpleType name="A"><xs:restriction base="t:C"/></xs:simpleType><xs:complexType name="C"/>"#,
        "A",
        Role::Both,
    );
    assert_eq!(codes, ["FLE7206"]);
}
