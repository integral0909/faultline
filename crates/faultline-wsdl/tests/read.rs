//! Reading WSDL 1.1 descriptions.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_source::{Limits, SourceMap};
use faultline_wsdl::Reader;
use faultline_wsdl::model::{SoapVersion, Style};
use faultline_xml::QName;

const PETS: &str = include_str!("data/pets.wsdl");

fn read(files: &[(&str, &str)]) -> (faultline_wsdl::model::Definitions, usize, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let sources = SourceMap::new();
    let mut reader = Reader::new(&loader, &sources, Limits::default());
    reader.load(&UriRef::parse(&format!("file:///{}", files[0].0)).unwrap());
    let mut codes: Vec<String> = reader
        .diagnostics
        .iter()
        .chain(reader.schemas.diagnostics.iter())
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    codes.sort();
    (
        reader.definitions,
        reader.schemas.components.elements.len(),
        codes,
    )
}

fn q(local: &str) -> QName {
    QName::new(Some("urn:example:pets"), local)
}

#[test]
fn definitions_are_read() {
    let (defs, elements, codes) = read(&[("pets.wsdl", PETS)]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(elements, 4, "the embedded schema's elements");
    assert_eq!(defs.target_namespace.as_deref(), Some("urn:example:pets"));
    assert_eq!(defs.messages.len(), 6);
    let count = defs.message(&q("CountRequest")).unwrap();
    let parts: Vec<(&str, bool)> = count
        .parts
        .iter()
        .map(|p| (p.name.as_str(), p.ty.is_some()))
        .collect();
    assert_eq!(parts, [("species", true), ("limit", true)]);

    let port_type = &defs.port_types[0];
    assert_eq!(port_type.name, q("PetPortType"));
    let get = &port_type.operations[0];
    assert_eq!(get.documentation.as_deref(), Some("Returns one pet."));
    assert_eq!(get.input.as_ref().unwrap().message, q("GetPetRequest"));
    assert_eq!(get.faults[0].name.as_deref(), Some("NotFound"));

    let bindings: Vec<(SoapVersion, Style)> = defs
        .bindings_of(&q("PetPortType"))
        .map(|b| (b.soap.unwrap(), b.style))
        .collect();
    assert_eq!(
        bindings,
        [
            (SoapVersion::V1_1, Style::Document),
            (SoapVersion::V1_2, Style::Document)
        ]
    );
    let soap = &defs.bindings[0];
    let bound = soap.operation("GetPet").unwrap();
    assert_eq!(
        bound.soap_action.as_deref(),
        Some("urn:example:pets/GetPet")
    );
    assert_eq!(bound.input.headers[0].part, "auth");
    assert_eq!(soap.operation("CountPets").unwrap().style, Some(Style::Rpc));
    assert_eq!(
        defs.services[0].ports[1].address.as_deref(),
        Some("https://pets.example.com/soap12")
    );
}

#[test]
fn imports_bring_in_descriptions_and_schemas() {
    let main = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" targetNamespace="urn:a">
      <wsdl:import namespace="urn:b" location="b.wsdl"/>
      <wsdl:import namespace="urn:c" location="schemas/c.xsd"/>
    </wsdl:definitions>"#;
    let b = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" targetNamespace="urn:b">
      <wsdl:import namespace="urn:a" location="main.wsdl"/>
      <wsdl:message name="M"/>
    </wsdl:definitions>"#;
    let c = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:c"><xs:element name="c" type="xs:string"/></xs:schema>"#;
    let (defs, elements, codes) = read(&[("main.wsdl", main), ("b.wsdl", b), ("schemas/c.xsd", c)]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(defs.messages[0].name, QName::new(Some("urn:b"), "M"));
    assert_eq!(elements, 1);
}

#[test]
fn problems_are_reported() {
    let codes = |text: &str| read(&[("t.wsdl", text)]).2;
    assert_eq!(codes("<xs:schema xmlns:xs='urn:not-xs'/>"), ["FLE7401"]);
    assert_eq!(
        codes(r#"<description xmlns="http://www.w3.org/ns/wsdl"/>"#),
        ["FLE7402"]
    );
    let head = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" targetNamespace="urn:t">"#;
    let doc = |body: &str| format!("{head}{body}</wsdl:definitions>");
    assert_eq!(
        codes(&doc(
            r#"<wsdl:message><wsdl:part name="p" type="x"/></wsdl:message>"#
        )),
        ["FLE7404"]
    );
    assert_eq!(
        codes(&doc(
            r#"<wsdl:message name="m"><wsdl:part name="p"/></wsdl:message>"#
        )),
        ["FLE7404"]
    );
    assert_eq!(codes(&doc(r#"<wsdl:binding name="b"/>"#)), ["FLE7404"]);
    assert_eq!(
        codes(&doc(r#"<wsdl:import location="missing.wsdl"/>"#)),
        ["FLE7205"]
    );
    assert_eq!(
        codes(&doc(r#"<wsdl:import location="../../etc/x.wsdl"/>"#)),
        ["FLE7405"]
    );
    assert_eq!(
        codes(&doc(r#"<wsdl:binding name="b" type="t:P"/>"#)),
        ["FLE7404"],
        "undeclared prefix"
    );
    assert_eq!(
        codes(&doc(
            r#"<wsdl:binding name="b" type="wsdl:P"><soap:binding style="rpc"/><wsdl:operation name="o"><wsdl:input><soap:body use="encoded"/></wsdl:input></wsdl:operation></wsdl:binding>"#
        )),
        ["FLE7406"]
    );
}
