//! Complex types and elements, end to end: lowering and comparison.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Role, TypeKind, validate};
use faultline_rules::{Finding, Severity, classify};
use faultline_source::{Limits, SourceMap};
use faultline_xsd::lower_xsd;

fn lowered(text: &str, role: Role) -> faultline_xsd::Lowered {
    let loader = MemoryLoader::new().with("file:///pets.xsd", text);
    let sources = SourceMap::new();
    lower_xsd(
        &UriRef::parse("file:///pets.xsd").unwrap(),
        &loader,
        &sources,
        "pets",
        role,
        Limits::default(),
    )
}

fn api(text: &str, role: Role) -> Api {
    let lowered = lowered(text, role);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn findings(old: &str, new: &str, role: Role) -> Vec<Finding> {
    faultline_diff::diff(&api(old, role), &api(new, role))
        .iter()
        .map(|c| classify("pets", c))
        .collect()
}

fn summary(found: &[Finding]) -> Vec<String> {
    found
        .iter()
        .map(|f| format!("{} {} {}", f.severity, f.rule.name, f.change.path))
        .collect()
}

fn worst(found: &[Finding]) -> Severity {
    found
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(Severity::Info)
}

const PETS: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:p="urn:pets" targetNamespace="urn:pets" elementFormDefault="qualified">
  <xs:element name="pet" type="p:Pet"/>
  <xs:element name="tree" type="p:Node"/>
  <xs:complexType name="Named">
    <xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence>
    <xs:attribute name="id" type="xs:long" use="required"/>
  </xs:complexType>
  <xs:complexType name="Pet">
    <xs:complexContent>
      <xs:extension base="p:Named">
        <xs:sequence>
          <xs:element name="tag" type="xs:string" minOccurs="0" maxOccurs="5"/>
          <xs:element name="weight" type="p:Weight" nillable="true"/>
          <xs:choice>
            <xs:element name="owner" type="xs:string"/>
            <xs:element name="shelter" type="xs:string"/>
          </xs:choice>
        </xs:sequence>
        <xs:attribute name="status" default="available">
          <xs:simpleType><xs:restriction base="xs:string">
            <xs:enumeration value="available"/><xs:enumeration value="sold"/>
          </xs:restriction></xs:simpleType>
        </xs:attribute>
      </xs:extension>
    </xs:complexContent>
  </xs:complexType>
  <xs:complexType name="Weight">
    <xs:simpleContent>
      <xs:extension base="xs:decimal"><xs:attribute name="unit" type="xs:string"/></xs:extension>
    </xs:simpleContent>
  </xs:complexType>
  <xs:complexType name="Node">
    <xs:sequence><xs:element name="child" type="p:Node" minOccurs="0" maxOccurs="unbounded"/></xs:sequence>
  </xs:complexType>
</xs:schema>"#;

#[test]
fn elements_become_objects() {
    let api = api(PETS, Role::Both);
    let roots: Vec<&str> = api.schemas.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(roots, ["pet", "tree"]);
    let pet = api.types.resolve(api.schemas[0].ty);
    let TypeKind::Object(object) = &api.types.get(pet).kind else {
        panic!()
    };
    let fields: Vec<(&str, bool)> = object
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        fields,
        [
            ("name", true),
            ("@id", true),
            ("tag", false),
            ("weight", true),
            ("owner", false),
            ("shelter", false),
            ("@status", false),
        ],
        "the base's members come first; choice options are optional"
    );
    assert_eq!(object.exclusive[0].fields, ["owner", "shelter"]);
    assert!(
        matches!(api.types.get(object.additional).kind, TypeKind::Never),
        "closed"
    );
    let tag = api.types.get(object.field("tag").unwrap().ty);
    assert!(matches!(&tag.kind, TypeKind::Array(a) if a.max_items == Some(5)));
    let weight = api.types.get(object.field("weight").unwrap().ty);
    let TypeKind::Union(nillable) = &weight.kind else {
        panic!()
    };
    let TypeKind::Object(simple_content) =
        &api.types.get(api.types.resolve(nillable.variants[0])).kind
    else {
        panic!()
    };
    let names: Vec<&str> = simple_content
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, ["#text", "@unit"]);
}

#[test]
fn requests_and_responses() {
    // A required element added: documents written before lack it.
    let added = PETS.replace(
        r#"<xs:element name="tag" type="xs:string" minOccurs="0" maxOccurs="5"/>"#,
        r#"<xs:element name="tag" type="xs:string" minOccurs="0" maxOccurs="5"/><xs:element name="breed" type="xs:string"/>"#,
    );
    assert_eq!(
        worst(&findings(PETS, &added, Role::Input)),
        Severity::Breaking
    );
    // More repetitions: readers built for five may get more.
    let more = PETS.replace(r#"maxOccurs="5""#, r#"maxOccurs="10""#);
    assert!(worst(&findings(PETS, &more, Role::Output)) >= Severity::Risky);
    assert!(worst(&findings(PETS, &more, Role::Input)) < Severity::Breaking);
    // An attribute that became required breaks writers.
    let required = PETS.replace(
        r#"<xs:attribute name="status" default="available">"#,
        r#"<xs:attribute name="status" use="required">"#,
    );
    let found = findings(PETS, &required, Role::Input);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(
        found
            .iter()
            .any(|f| f.change.path.to_string().ends_with(".@status"))
    );
    // An unchanged schema has no findings.
    assert!(findings(PETS, PETS, Role::Both).is_empty());
}

#[test]
fn problems_are_reported() {
    let codes = |text: &str| -> Vec<String> {
        lowered(text, Role::Both)
            .diagnostics
            .iter()
            .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
            .collect()
    };
    let schema = |body: &str| {
        format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:p="urn:p" targetNamespace="urn:p">{body}</xs:schema>"#
        )
    };
    assert_eq!(
        codes(&schema(r#"<xs:element name="a" type="p:Missing"/>"#)),
        ["FLE7204"]
    );
    assert_eq!(
        codes(&schema(
            r#"<xs:element name="a"><xs:complexType><xs:sequence><xs:element ref="p:missing"/></xs:sequence></xs:complexType></xs:element>"#
        )),
        ["FLE7204"]
    );
    assert_eq!(
        codes(&schema(
            r#"<xs:element name="a" type="p:A"/><xs:complexType name="A"><xs:complexContent><xs:extension base="p:A"/></xs:complexContent></xs:complexType>"#
        )),
        ["FLE7206"]
    );
    assert_eq!(
        codes(&schema(
            r#"<xs:element name="a" type="p:A"/><xs:complexType name="A"><xs:group ref="p:G"/></xs:complexType><xs:group name="G"><xs:sequence><xs:group ref="p:G"/></xs:sequence></xs:group>"#
        )),
        ["FLE7206"]
    );
    // Errors while reading stop lowering.
    let broken = lowered("<xs:schema", Role::Both);
    assert!(broken.api.is_none());
}

fn order_schema(group: &str, elements: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType><xs:{group}>{elements}</xs:{group}></xs:complexType>
  </xs:element>
</xs:schema>"#
    )
}

const ID: &str = r#"<xs:element name="id" type="xs:string"/>"#;
const QTY: &str = r#"<xs:element name="qty" type="xs:int"/>"#;
const NOTE: &str = r#"<xs:element name="note" type="xs:string" minOccurs="0"/>"#;

#[test]
fn sequences_fix_the_order_of_their_elements() {
    let old = order_schema("sequence", &format!("{ID}{QTY}"));
    let swapped = order_schema("sequence", &format!("{QTY}{ID}"));
    let found = findings(&old, &swapped, Role::Input);
    assert_eq!(
        summary(&found),
        ["breaking member-order-changed schema[order]"]
    );
    assert!(
        found[0].message.contains("qty, id instead of id, qty"),
        "{}",
        found[0].message
    );
    assert_eq!(
        worst(&findings(&old, &swapped, Role::Output)),
        Severity::Risky,
        "clients that validate reject the new order"
    );

    // Adding an element at the end keeps the order of the others.
    let appended = order_schema("sequence", &format!("{ID}{QTY}{NOTE}"));
    assert!(
        !summary(&findings(&old, &appended, Role::Input))
            .iter()
            .any(|s| s.contains("member-order")),
    );
    // `all` does not fix an order.
    let all = order_schema("all", &format!("{ID}{QTY}"));
    let all_swapped = order_schema("all", &format!("{QTY}{ID}"));
    assert!(findings(&all, &all_swapped, Role::Input).is_empty());
}
