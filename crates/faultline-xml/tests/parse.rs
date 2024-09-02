//! Well-formedness, namespaces, spans and the refusal of DTDs.

use faultline_source::{Diagnostics, Limits, SourceId, SourceMap, SourceName};
use faultline_xml::{Content, Document, QName, parse};

fn read(text: &str) -> (Option<Document>, Vec<String>) {
    read_with(text, &Limits::default())
}

fn read_with(text: &str, limits: &Limits) -> (Option<Document>, Vec<String>) {
    let mut diagnostics = Diagnostics::new();
    let document = parse(text, SourceId::from_raw(0), limits, &mut diagnostics);
    let codes = diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (document, codes)
}

fn document(text: &str) -> Document {
    let (document, codes) = read(text);
    assert!(codes.is_empty(), "{codes:?}");
    document.unwrap()
}

const XS: &str = "http://www.w3.org/2001/XMLSchema";

#[test]
fn namespaces_resolve_by_scope() {
    let doc = document(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- a schema -->
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns="urn:default" xmlns:tns="urn:pets" targetNamespace="urn:pets">
  <xs:element name="pet" type="tns:Pet"/>
  <plain xmlns="">
    <inner a:b="1" xmlns:a="urn:a"/>
  </plain>
  <other/>
</xs:schema>
"#,
    );
    let root = &doc.root;
    assert_eq!(root.name, QName::new(Some(XS), "schema"));
    assert_eq!(root.raw_name, "xs:schema");
    assert_eq!(root.attr("targetNamespace"), Some("urn:pets"));
    assert!(
        root.attribute("xmlns").is_none(),
        "declarations are not attributes"
    );

    let element = root.child(XS, "element").unwrap();
    assert_eq!(
        element.resolve_qname(element.attr("type").unwrap()),
        Some(QName::new(Some("urn:pets"), "Pet"))
    );
    assert_eq!(element.resolve_qname("undeclared:Pet"), None);

    let children: Vec<&faultline_xml::Element> = root.elements().collect();
    let plain = children[1];
    assert_eq!(
        plain.name,
        QName::new(None, "plain"),
        "xmlns=\"\" undeclares the default"
    );
    let inner = plain.elements().next().unwrap();
    assert_eq!(inner.attributes[0].name, QName::new(Some("urn:a"), "b"));
    assert_eq!(children[2].name, QName::new(Some("urn:default"), "other"));
}

#[test]
fn text_references_and_cdata() {
    let doc = document(
        "<a x='1 &lt; 2&#x9;&#65;' y='a\tb\r\nc'>fish &amp; chips<!-- c --> and <![CDATA[<raw> & more]]>\r\n<b/>  </a>",
    );
    // Character references survive attribute-value normalization.
    assert_eq!(doc.root.attr("x"), Some("1 < 2\tA"));
    assert_eq!(
        doc.root.attr("y"),
        Some("a b c"),
        "literal whitespace is normalized"
    );
    assert_eq!(doc.root.text(), "fish & chips and <raw> & more\n");
    assert_eq!(
        doc.root.children.len(),
        2,
        "whitespace-only runs are dropped"
    );
    assert!(matches!(doc.root.children[1], Content::Element(_)));
}

#[test]
fn spans_point_at_constructs() {
    let text = "<root>\n  <item name=\"x\"/>\n</root>";
    let map = SourceMap::new();
    let file = map.add(SourceName::Virtual("t".into()), text).unwrap();
    let mut diagnostics = Diagnostics::new();
    let doc = parse(text, file.id(), &Limits::default(), &mut diagnostics).unwrap();
    let item = doc.root.elements().next().unwrap();
    assert_eq!(map.slice(item.span).unwrap(), "<item name=\"x\"/>");
    assert_eq!(map.slice(item.name_span).unwrap(), "item");
    assert_eq!(map.slice(item.attributes[0].span).unwrap(), "name=\"x\"");
    assert_eq!(map.slice(item.attributes[0].value_span).unwrap(), "x");
    assert_eq!(map.slice(doc.root.span).unwrap(), text);
}

#[test]
fn dtds_are_refused() {
    let billion_laughs = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;">]>
<lolz>&lol2;</lolz>"#;
    assert_eq!(read(billion_laughs).1, ["FLE0702"]);
    let external = r#"<!DOCTYPE foo [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><foo>&xxe;</foo>"#;
    assert_eq!(read(external).1, ["FLE0702"]);
    assert_eq!(read("<a>&xxe;</a>").1, ["FLE0703"]);
}

#[test]
fn malformed_documents() {
    for (text, code) in [
        ("", "FLE0701"),
        ("<a>", "FLE0701"),
        ("<a></b>", "FLE0706"),
        ("<a b='1' b='2'/>", "FLE0705"),
        ("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>", "FLE0705"),
        ("<p:a/>", "FLE0704"),
        ("<a p:b='1'/>", "FLE0704"),
        ("<a/><b/>", "FLE0701"),
        ("<a b=1/>", "FLE0701"),
        ("<a b='<'/>", "FLE0701"),
        ("<a>&#0;</a>", "FLE0703"),
        ("<a>&#xD800;</a>", "FLE0703"),
        ("<a>& b</a>", "FLE0703"),
        ("<a><!-- open</a>", "FLE0701"),
        ("<a><![CDATA[x</a>", "FLE0701"),
        ("<a><!ELEMENT x></a>", "FLE0701"),
        ("text", "FLE0701"),
    ] {
        let (document, codes) = read(text);
        assert!(document.is_none(), "{text:?}");
        assert_eq!(codes, [code], "{text:?}");
    }
}

#[test]
fn limits_hold() {
    let limits = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    assert_eq!(
        read_with("<a><b><c/></b></a>", &limits).1,
        Vec::<String>::new()
    );
    assert_eq!(
        read_with("<a><b><c><d/></c></b></a>", &limits).1,
        ["FLE0707"]
    );
    let deep = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
    assert_eq!(read(&deep).1, ["FLE0707"], "no stack overflow");
    let limits = Limits {
        max_document_nodes: 3,
        ..Limits::default()
    };
    assert_eq!(read_with("<a><b/><c/><d/></a>", &limits).1, ["FLE0708"]);
}
