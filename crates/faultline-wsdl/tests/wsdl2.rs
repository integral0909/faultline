//! WSDL 2.0 descriptions, read into the same model as WSDL 1.1.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, validate};
use faultline_source::{Limits, SourceMap};
use faultline_wsdl::lower_wsdl;

fn lowered(uri: &str, text: &str) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new().with(uri, text);
    let sources = SourceMap::new();
    let lowered = lower_wsdl(
        &UriRef::parse(uri).unwrap(),
        &loader,
        &sources,
        "pets",
        Limits::default(),
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

const SCHEMA: &str = r#"<xs:schema targetNamespace="urn:example:pets" elementFormDefault="qualified">
      <xs:element name="GetPet">
        <xs:complexType><xs:sequence><xs:element name="id" type="xs:long"/></xs:sequence></xs:complexType>
      </xs:element>
      <xs:element name="GetPetResponse">
        <xs:complexType><xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence></xs:complexType>
      </xs:element>
      <xs:element name="NotFound">
        <xs:complexType><xs:sequence><xs:element name="id" type="xs:long"/></xs:sequence></xs:complexType>
      </xs:element>
    </xs:schema>"#;

fn wsdl11() -> String {
    format!(
        r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/"
    xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/"
    xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:example:pets"
    targetNamespace="urn:example:pets">
  <wsdl:types>{SCHEMA}</wsdl:types>
  <wsdl:message name="In"><wsdl:part name="body" element="tns:GetPet"/></wsdl:message>
  <wsdl:message name="Out"><wsdl:part name="body" element="tns:GetPetResponse"/></wsdl:message>
  <wsdl:message name="Fault"><wsdl:part name="body" element="tns:NotFound"/></wsdl:message>
  <wsdl:portType name="Pets">
    <wsdl:operation name="GetPet">
      <wsdl:input message="tns:In"/>
      <wsdl:output message="tns:Out"/>
      <wsdl:fault name="NotFound" message="tns:Fault"/>
    </wsdl:operation>
  </wsdl:portType>
  <wsdl:binding name="PetsSoap" type="tns:Pets">
    <soap12:binding style="document"/>
    <wsdl:operation name="GetPet">
      <soap12:operation soapAction="urn:GetPet"/>
      <wsdl:input><soap12:body use="literal"/></wsdl:input>
      <wsdl:output><soap12:body use="literal"/></wsdl:output>
    </wsdl:operation>
  </wsdl:binding>
</wsdl:definitions>"#
    )
}

fn wsdl20(extra_interface: &str, extra_binding: &str) -> String {
    format!(
        r#"<description xmlns="http://www.w3.org/ns/wsdl"
    xmlns:wsoap="http://www.w3.org/ns/wsdl/soap"
    xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:example:pets"
    targetNamespace="urn:example:pets">
  <types>{SCHEMA}</types>
  <interface name="Pets">
    <fault name="NotFound" element="tns:NotFound"/>
    <operation name="GetPet" pattern="http://www.w3.org/ns/wsdl/in-out">
      <input messageLabel="In" element="tns:GetPet"/>
      <output messageLabel="Out" element="tns:GetPetResponse"/>
      <outfault ref="tns:NotFound" messageLabel="Out"/>
    </operation>
    {extra_interface}
  </interface>
  <binding name="PetsSoap" interface="tns:Pets" type="http://www.w3.org/ns/wsdl/soap">
    <operation ref="tns:GetPet" wsoap:action="urn:GetPet"/>
    {extra_binding}
  </binding>
  <service name="PetService" interface="tns:Pets">
    <endpoint name="Soap" binding="tns:PetsSoap" address="https://pets.example.com/soap"/>
  </service>
</description>"#
    )
}

#[test]
fn a_wsdl_2_description_is_its_wsdl_1_1_equivalent() {
    let (old, codes) = lowered("file:///pets11.wsdl", &wsdl11());
    assert!(codes.is_empty(), "{codes:?}");
    let (new, codes) = lowered("file:///pets20.wsdl", &wsdl20("", ""));
    assert!(codes.is_empty(), "{codes:?}");
    let (old, new) = (old.unwrap(), new.unwrap());
    assert_eq!(validate(&new), []);
    let keys: Vec<String> = new.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(keys, ["Pets/GetPet"]);
    let changes = faultline_diff::diff(&old, &new);
    assert!(changes.is_empty(), "{:?}", changes.changes);
}

#[test]
fn wsdl_2_patterns_and_extensions() {
    let text = r##"<description xmlns="http://www.w3.org/ns/wsdl"
    xmlns:wsoap="http://www.w3.org/ns/wsdl/soap" xmlns:tns="urn:t" targetNamespace="urn:t">
  <interface name="Base">
    <operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-only">
      <input element="#none"/>
    </operation>
  </interface>
  <interface name="Main" extends="tns:Base">
    <operation name="Echo" pattern="http://www.w3.org/ns/wsdl/in-out">
      <input element="#any"/>
      <output element="#any"/>
    </operation>
  </interface>
  <binding name="Soap11" interface="tns:Main" type="http://www.w3.org/ns/wsdl/soap" wsoap:version="1.1">
    <operation ref="tns:Echo" wsoap:action="urn:Echo"/>
  </binding>
</description>"##;
    let (api, codes) = lowered("file:///t.wsdl", text);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let mut keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["Base/Ping", "Main/Echo", "Main/Ping"],
        "inherited through extends"
    );
    let ping = api
        .operations
        .iter()
        .find(|o| o.key.to_string() == "Main/Ping")
        .unwrap();
    assert!(ping.one_way, "in-only");
    let echo = api
        .operations
        .iter()
        .find(|o| o.key.to_string() == "Main/Echo")
        .unwrap();
    assert_eq!(
        echo.request_body.as_ref().unwrap().content[0].media_type,
        "text/xml"
    );
    assert!(
        echo.parameters
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case("SOAPAction")),
        "SOAP 1.1 sends the action as a header"
    );
}

#[test]
fn unknown_interfaces_are_reported() {
    let text = r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:tns="urn:t" targetNamespace="urn:t">
  <interface name="Main" extends="tns:Missing"/>
</description>"#;
    let (api, codes) = lowered("file:///t.wsdl", text);
    assert!(api.is_none());
    assert_eq!(codes, ["FLE7403"]);
}
