//! WSDL services lowered and compared.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ApiFormat, ParamLocation, TypeKind, validate};
use faultline_rules::{Finding, Severity, classify};
use faultline_source::{Limits, SourceMap};
use faultline_wsdl::lower_wsdl;

const PETS: &str = include_str!("data/pets.wsdl");

fn lowered(text: &str) -> faultline_wsdl::Lowered {
    let loader = MemoryLoader::new().with("file:///pets.wsdl", text);
    let sources = SourceMap::new();
    lower_wsdl(
        &UriRef::parse("file:///pets.wsdl").unwrap(),
        &loader,
        &sources,
        "pets",
        Limits::default(),
    )
}

fn api(text: &str) -> Api {
    let lowered = lowered(text);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn findings(new: &str) -> Vec<Finding> {
    faultline_diff::diff(&api(PETS), &api(new))
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

#[test]
fn operations_carry_soap_messages() {
    let api = api(PETS);
    assert_eq!(api.format, ApiFormat::Wsdl);
    let ids: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(ids, ["PetPortType/GetPet", "PetPortType/CountPets"]);

    let get = &api.operations[0];
    let headers: Vec<(&str, ParamLocation)> = get
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();
    assert_eq!(
        headers,
        [
            ("SOAPAction", ParamLocation::Header),
            ("Auth", ParamLocation::Header)
        ]
    );
    let body = get.request_body.as_ref().unwrap();
    let media: Vec<&str> = body.content.iter().map(|c| c.media_type.as_str()).collect();
    assert_eq!(
        media,
        [
            "text/xml",
            "application/soap+xml; action=\"urn:example:pets/GetPet\""
        ],
        "one per SOAP binding, with the SOAP 1.2 action"
    );
    let TypeKind::Object(request) = &api.types.get(api.types.resolve(body.content[0].ty)).kind
    else {
        panic!()
    };
    assert_eq!(
        request.fields[0].name, "id",
        "a document body is its element"
    );
    let statuses: Vec<String> = get.responses.iter().map(|r| r.status.to_string()).collect();
    assert_eq!(statuses, ["200", "500"]);

    // RPC style wraps the parts in an element named after the operation.
    let count = &api.operations[1];
    let TypeKind::Object(rpc) = &api
        .types
        .get(count.request_body.as_ref().unwrap().content[0].ty)
        .kind
    else {
        panic!()
    };
    assert_eq!(rpc.fields[0].name, "CountPets");
    let TypeKind::Object(output) = &api.types.get(count.responses[0].content[0].ty).kind else {
        panic!()
    };
    assert_eq!(output.fields[0].name, "CountPetsResponse");
}

#[test]
fn an_unchanged_service_has_no_findings() {
    assert!(findings(PETS).is_empty());
}

#[test]
fn contract_changes_are_classified() {
    // A new required request element breaks every existing client.
    let required = PETS.replace(
        r#"<xs:element name="id" type="xs:long"/></xs:sequence></xs:complexType>
      </xs:element>
      <xs:element name="GetPetResponse">"#,
        r#"<xs:element name="id" type="xs:long"/><xs:element name="locale" type="xs:string"/></xs:sequence></xs:complexType>
      </xs:element>
      <xs:element name="GetPetResponse">"#,
    );
    let found = findings(&required);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));

    // SOAP 1.1 clients send the old SOAPAction header.
    let action = PETS.replace(
        r#"<soap:operation soapAction="urn:example:pets/GetPet"/>"#,
        r#"<soap:operation soapAction="urn:example:pets/v2/GetPet"/>"#,
    );
    let found = findings(&action);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
    assert!(found.iter().any(|f| {
        f.change
            .path
            .to_string()
            .contains("param[header:soapaction]")
    }));

    // Clients of the SOAP 1.2 endpoint lose it.
    let start = PETS.find(r#"<wsdl:binding name="PetSoap12""#).unwrap();
    let end = PETS[start..].find("</wsdl:binding>").unwrap() + start + "</wsdl:binding>".len();
    let without_12 = format!("{}{}", &PETS[..start], &PETS[end..]).replace(
        r#"<wsdl:port name="PetPort12" binding="tns:PetSoap12">
      <soap12:address location="https://pets.example.com/soap12"/>
    </wsdl:port>"#,
        "",
    );
    let found = findings(&without_12);
    assert!(
        found
            .iter()
            .any(|f| f.severity == Severity::Breaking && f.rule.name == "media-type-removed"),
        "{:?}",
        summary(&found)
    );

    // A new fault is a response clients may not handle.
    let fault = PETS.replace(
        r#"<wsdl:fault name="NotFound" message="tns:NotFoundFault"/>
    </wsdl:operation>"#,
        r#"<wsdl:fault name="NotFound" message="tns:NotFoundFault"/>
      <wsdl:fault name="Throttled" message="tns:NotFoundFault"/>
    </wsdl:operation>"#,
    );
    let found = findings(&fault);
    assert!(worst(&found) >= Severity::Risky, "{:?}", summary(&found));
    assert!(
        found
            .iter()
            .all(|f| f.change.path.to_string().contains("response[500]"))
    );

    // A narrower RPC part rejects values clients send.
    let narrower = PETS.replace(
        r#"<wsdl:part name="limit" type="xs:int"/>"#,
        r#"<wsdl:part name="limit" type="xs:short"/>"#,
    );
    assert_eq!(worst(&findings(&narrower)), Severity::Breaking);
    let wider = PETS.replace(
        r#"<wsdl:part name="limit" type="xs:int"/>"#,
        r#"<wsdl:part name="limit" type="xs:long"/>"#,
    );
    assert!(worst(&findings(&wider)) < Severity::Breaking);

    // A removed operation.
    let start = PETS.find(r#"<wsdl:operation name="CountPets">"#).unwrap();
    let end = PETS[start..].find("</wsdl:operation>").unwrap() + start + "</wsdl:operation>".len();
    let removed = format!("{}{}", &PETS[..start], &PETS[end..]);
    let found = findings(&removed);
    assert!(
        found
            .iter()
            .any(|f| f.rule.name == "operation-removed" && f.severity == Severity::Breaking),
        "{:?}",
        summary(&found)
    );
}

#[test]
fn unresolved_references_are_errors() {
    let codes = |text: &str| -> Vec<String> {
        lowered(text)
            .diagnostics
            .iter()
            .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
            .collect()
    };
    let missing_message = PETS.replace(
        r#"<wsdl:output message="tns:CountResponse"/>"#,
        r#"<wsdl:output message="tns:Nothing"/>"#,
    );
    assert_eq!(codes(&missing_message), ["FLE7403"]);
    let missing_element = PETS.replace(r#"element="tns:NotFound""#, r#"element="tns:Gone""#);
    assert_eq!(codes(&missing_element), ["FLE7403"]);
    assert!(lowered(&missing_element).api.is_none());
}

#[test]
fn one_way_operations_are_marked() {
    let one_way = PETS.replace(r#"<wsdl:output message="tns:CountResponse"/>"#, "");
    let api = api(&one_way);
    let count = api
        .operations
        .iter()
        .find(|o| o.key.to_string() == "PetPortType/CountPets")
        .unwrap();
    assert!(count.one_way);
    assert!(count.responses.is_empty());
    let found = findings(&one_way);
    assert_eq!(
        summary(&found),
        ["breaking one-way-changed op[PetPortType/CountPets]"],
        "the removed response is part of the change"
    );
}
