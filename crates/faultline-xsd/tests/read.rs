//! Reading schema documents into components.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_source::{Limits, SourceMap};
use faultline_xsd::model::{
    AttributeUse, ContentModel, Derivation, Occurs, QName, SimpleVariety, Term, TypeDef, TypeRef,
};
use faultline_xsd::{Reader, XS};

fn read(files: &[(&str, &str)]) -> (faultline_xsd::model::Components, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let sources = SourceMap::new();
    let mut reader = Reader::new(&loader, &sources, Limits::default());
    reader.load(&UriRef::parse(&format!("file:///{}", files[0].0)).unwrap());
    let codes = reader
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (reader.components, codes)
}

const PETS: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:tns="urn:pets" targetNamespace="urn:pets" elementFormDefault="qualified">
  <xs:include schemaLocation="common.xsd"/>
  <xs:element name="pet" type="tns:Pet">
    <xs:annotation><xs:documentation>A pet.</xs:documentation></xs:annotation>
  </xs:element>
  <xs:complexType name="Pet">
    <xs:complexContent>
      <xs:extension base="tns:Animal">
        <xs:sequence>
          <xs:element name="name" type="tns:Name"/>
          <xs:element name="tag" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>
          <xs:choice>
            <xs:element ref="tns:owner"/>
            <xs:group ref="tns:Shelter"/>
          </xs:choice>
          <xs:any minOccurs="0"/>
        </xs:sequence>
        <xs:attribute name="id" type="xs:long" use="required"/>
        <xs:attributeGroup ref="tns:Audit"/>
        <xs:anyAttribute/>
      </xs:extension>
    </xs:complexContent>
  </xs:complexType>
  <xs:element name="owner" type="xs:string" nillable="true"/>
  <xs:group name="Shelter"><xs:sequence><xs:element name="shelter" type="xs:string"/></xs:sequence></xs:group>
  <xs:attributeGroup name="Audit"><xs:attribute name="created" type="xs:dateTime"/></xs:attributeGroup>
  <xs:complexType name="Animal"><xs:sequence/></xs:complexType>
</xs:schema>"#;

const COMMON: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:simpleType name="Name">
    <xs:restriction base="xs:string">
      <xs:minLength value="1"/><xs:maxLength value="64"/>
      <xs:pattern value="[a-z]+"/><xs:pattern value="[A-Z]+"/>
    </xs:restriction>
  </xs:simpleType>
  <xs:simpleType name="Tags"><xs:list itemType="xs:token"/></xs:simpleType>
  <xs:simpleType name="Id"><xs:union memberTypes="xs:int xs:string"/></xs:simpleType>
</xs:schema>"#;

fn q(local: &str) -> QName {
    QName::new(Some("urn:pets"), local)
}

#[test]
fn components_are_collected_across_includes() {
    let (components, codes) = read(&[("pets.xsd", PETS), ("common.xsd", COMMON)]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(components.element_order, [q("pet"), q("owner")]);
    let pet = &components.elements[&q("pet")];
    assert_eq!(pet.documentation.as_deref(), Some("A pet."));
    assert!(matches!(&pet.ty, Some(TypeRef::Named(name, _)) if *name == q("Pet")));
    assert!(components.elements[&q("owner")].nillable);

    // The included schema has no namespace and takes the includer's.
    let TypeDef::Simple(name) = &components.types[&q("Name")] else {
        panic!()
    };
    let SimpleVariety::Restriction { base, facets } = &name.variety else {
        panic!()
    };
    assert!(matches!(base, TypeRef::Named(b, _) if *b == QName::new(Some(XS), "string")));
    assert_eq!((facets.min_length, facets.max_length), (Some(1), Some(64)));
    assert_eq!(
        facets.patterns,
        ["[a-z]+|[A-Z]+"],
        "one step's patterns are alternatives"
    );
    assert!(
        matches!(&components.types[&q("Tags")], TypeDef::Simple(t) if matches!(t.variety, SimpleVariety::List { .. }))
    );
    assert!(
        matches!(&components.types[&q("Id")], TypeDef::Simple(t) if matches!(&t.variety, SimpleVariety::Union { members } if members.len() == 2))
    );

    let TypeDef::Complex(pet_type) = &components.types[&q("Pet")] else {
        panic!()
    };
    assert_eq!(
        pet_type.base.as_ref().map(|(d, n, _)| (*d, n.clone())),
        Some((Derivation::Extension, q("Animal")))
    );
    assert!(pet_type.any_attribute);
    assert!(
        matches!(&pet_type.attributes[0], AttributeUse::Declared { name, required: true, .. } if name.local == "id" && name.namespace.is_none())
    );
    assert!(
        matches!(&pet_type.attributes[1], AttributeUse::GroupRef(name, _) if *name == q("Audit"))
    );
    let ContentModel::Elements(particle) = &pet_type.content else {
        panic!()
    };
    let Term::Sequence(items) = &particle.term else {
        panic!()
    };
    assert!(
        matches!(&items[0].term, Term::Element(e) if e.name == q("name")),
        "qualified locals"
    );
    assert_eq!(items[1].occurs, Occurs { min: 0, max: None });
    let Term::Choice(options) = &items[2].term else {
        panic!()
    };
    assert!(matches!(&options[0].term, Term::ElementRef(r) if *r == q("owner")));
    assert!(matches!(&options[1].term, Term::GroupRef(r) if *r == q("Shelter")));
    assert!(matches!(items[3].term, Term::Any));
    assert!(components.groups.contains_key(&q("Shelter")));
    assert!(components.attribute_groups.contains_key(&q("Audit")));
}

#[test]
fn unqualified_locals_have_no_namespace() {
    let text = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:pets">
      <xs:complexType name="T"><xs:sequence>
        <xs:element name="a" type="xs:string"/>
        <xs:element name="b" type="xs:string" form="qualified"/>
      </xs:sequence></xs:complexType>
    </xs:schema>"#;
    let (components, _) = read(&[("t.xsd", text)]);
    let TypeDef::Complex(t) = &components.types[&q("T")] else {
        panic!()
    };
    let ContentModel::Elements(p) = &t.content else {
        panic!()
    };
    let Term::Sequence(items) = &p.term else {
        panic!()
    };
    let names: Vec<&QName> = items
        .iter()
        .map(|i| match &i.term {
            Term::Element(e) => &e.name,
            _ => panic!(),
        })
        .collect();
    assert_eq!(names, [&QName::new(None, "a"), &q("b")]);
}

#[test]
fn problems_are_reported() {
    let schema = |body: &str| {
        format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:pets">{body}</xs:schema>"#
        )
    };
    let codes = |text: &str| read(&[("t.xsd", text)]).1;
    assert_eq!(codes("<definitions/>"), ["FLE7201"]);
    assert_eq!(
        codes(&schema(r#"<xs:element name="a"/><xs:element name="a"/>"#)),
        ["FLE7203"]
    );
    assert_eq!(
        codes(&schema(r#"<xs:element name="a" type="p:T"/>"#)),
        ["FLE7202"]
    );
    assert_eq!(
        codes(&schema(r#"<xs:include schemaLocation="missing.xsd"/>"#)),
        ["FLE7205"]
    );
    assert_eq!(
        codes(&schema(r#"<xs:include schemaLocation="../../etc/x.xsd"/>"#)),
        ["FLE7205"]
    );
    assert_eq!(
        codes(&schema(
            r#"<xs:element name="a" type="xs:string" minOccurs="x"/>"#
        )),
        Vec::<String>::new(),
        "global elements have no occurrence"
    );
    assert_eq!(
        codes(&schema(
            r#"<xs:complexType name="T"><xs:sequence><xs:element name="a" minOccurs="many"/></xs:sequence></xs:complexType>"#
        )),
        ["FLE7206"]
    );
    assert_eq!(
        codes(&schema(r#"<xs:redefine schemaLocation="t.xsd"/>"#)),
        ["FLE7207"]
    );
    assert_eq!(
        codes("<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><!DOCTYPE"),
        ["FLE0701"]
    );
}

#[test]
fn documents_are_read_once() {
    let a = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:a"><xs:import namespace="urn:b" schemaLocation="b.xsd"/><xs:element name="a" type="xs:string"/></xs:schema>"#;
    let b = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:b"><xs:import namespace="urn:a" schemaLocation="a.xsd"/><xs:element name="b" type="xs:string"/></xs:schema>"#;
    let (components, codes) = read(&[("a.xsd", a), ("b.xsd", b)]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(components.elements.len(), 2, "import cycles terminate");
}
