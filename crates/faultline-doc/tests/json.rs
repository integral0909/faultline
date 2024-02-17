mod common;

use common::{json, parse_with, to_json};
use faultline_doc::{Decimal, Format, Kind, Node, Number, Object, Value};
use faultline_source::{DiagnosticCode, Limits, SourceId, Span, render};
use proptest::prelude::*;

#[test]
fn values_and_spans() {
    let out = json(r#" {"name": "pet", "tags": [1, true, null, -2.5e3], "empty": {}} "#);
    out.assert_clean();
    let root = out.root();
    assert_eq!(out.slice(root.span), &out.text[1..out.text.len() - 1]);

    let obj = root.as_object().unwrap();
    assert_eq!(obj.keys().collect::<Vec<_>>(), ["name", "tags", "empty"]);
    assert_eq!(out.slice(obj.member("tags").unwrap().key_span), "\"tags\"");

    let tags = root.get("tags").unwrap();
    assert_eq!(out.slice(tags.span), "[1, true, null, -2.5e3]");
    let kinds: Vec<Kind> = tags.as_array().unwrap().iter().map(Node::kind).collect();
    assert_eq!(kinds, [Kind::Number, Kind::Bool, Kind::Null, Kind::Number]);
    assert_eq!(
        tags.as_array().unwrap()[3].as_number(),
        Some(&Number::Finite(Decimal::parse("-2500").unwrap()))
    );
    assert_eq!(out.slice(root.get("name").unwrap().span), "\"pet\"");
}

#[test]
fn string_escapes_are_decoded() {
    let out = json(r#"["\"\\\/\b\f\n\r\t", "\u00e9\u20AC", "\ud83d\ude00", "plain"]"#);
    out.assert_clean();
    let items: Vec<&str> = out
        .root()
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(items, ["\"\\/\u{8}\u{c}\n\r\t", "é€", "😀", "plain"]);
}

#[test]
fn lone_surrogates_are_replaced_with_a_warning() {
    let out = json(r#"["a\ud800b", "\udc00", "\ud800\u0041"]"#);
    let items: Vec<&str> = out
        .root()
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(items, ["a\u{FFFD}b", "\u{FFFD}", "\u{FFFD}A"]);
    assert_eq!(out.codes(), ["FLE0104", "FLE0104", "FLE0104"]);
    assert!(!out.parsed.diagnostics.has_errors());
}

#[test]
fn numbers_keep_full_precision() {
    let out = json("[0.1, 0.10000000000000001, 12345678901234567890123, 1E2, -0]");
    let n: Vec<&Decimal> = out
        .root()
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_number().unwrap().as_decimal().unwrap())
        .collect();
    assert_ne!(n[0], n[1]);
    assert_eq!(n[2].to_string(), "12345678901234567890123");
    assert_eq!(n[3].to_i64(), Some(100));
    assert!(n[4].is_zero());
}

/// Each case: input, expected code, expected primary span text, and a
/// fragment the message or help must contain.
#[test]
fn syntax_errors_point_at_the_problem() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (r#"{"a": 1,}"#, "FLE0101", ",", "remove this comma"),
        ("[1, 2,]", "FLE0101", ",", "remove this comma"),
        ("{a: 1}", "FLE0101", "a", "double quotes"),
        ("{'a': 1}", "FLE0101", "'", "double quotes"),
        ("// note\n{}", "FLE0101", "/", "comments"),
        ("[NaN]", "FLE0101", "N", "NaN"),
        ("[Infinity]", "FLE0101", "I", "Infinity"),
        ("[01]", "FLE0106", "01", "leading zeros"),
        ("[1.]", "FLE0106", "1.", "decimal point"),
        ("[-]", "FLE0106", "-", "expected a digit"),
        ("[1e+]", "FLE0106", "1e+", "exponent"),
        (r#""abc"#, "FLE0102", "", "unterminated"),
        (r#"["a\qb"]"#, "FLE0103", "\\q", "valid escapes"),
        (r#"["\u12G4"]"#, "FLE0103", "\\u12G4", "hexadecimal"),
        ("[\"a\tb\"]", "FLE0105", "\t", "escape"),
        ("{} {}", "FLE0107", "{}", "end of input"),
        ("", "FLE0102", "", "end of input"),
        ("   \n  ", "FLE0102", "", "end of input"),
        (r#"{"a" 1}"#, "FLE0101", "1", "':'"),
        ("[1 2]", "FLE0101", "2", "',' or ']'"),
        (r#"{"a": 1 "b": 2}"#, "FLE0101", "\"", "',' or '}'"),
        ("[truex]", "FLE0101", "x", "',' or ']'"),
        ("[tru]", "FLE0101", "t", "a value"),
        ("[1e999999999]", "FLE0006", "1e999999999", "out of range"),
        ("[é]", "FLE0101", "é", "a value"),
    ];
    for &(input, code, span_text, fragment) in cases {
        let out = json(input);
        assert!(out.parsed.document.is_none(), "{input:?} should fail");
        let d = out.only_diagnostic();
        assert_eq!(d.code.map(DiagnosticCode::as_str), Some(code), "{input:?}");
        assert_eq!(out.primary_text(d), span_text, "{input:?}");
        let text = format!("{} {:?} {:?}", d.message, d.labels, d.help);
        assert!(text.contains(fragment), "{input:?}: {text}");
    }
}

#[test]
fn duplicate_keys_are_errors_but_parsing_continues() {
    let out = json(r#"{"a": 1, "b": {"x": 1, "x": 2}, "a": 3}"#);
    assert_eq!(out.codes(), ["FLE0005", "FLE0005"]);
    let root = out.root();
    assert_eq!(root.get("a").unwrap().as_number().unwrap().to_string(), "3");
    // The replacement keeps the original position.
    assert_eq!(
        root.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["a", "b"]
    );
    let first = out.parsed.diagnostics.iter().next().unwrap();
    assert_eq!(first.labels.len(), 2);
    assert_eq!(out.slice(first.labels[1].span), "\"x\"");
}

#[test]
fn depth_limit_is_enforced() {
    let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
    let out = json(&deep);
    assert_eq!(out.codes(), ["FLE0003"]);

    let limits = Limits {
        max_depth: 3,
        ..Limits::default()
    };
    assert!(
        parse_with(Format::Json, "[[[1]]]", &limits)
            .parsed
            .document
            .is_some()
    );
    assert_eq!(
        parse_with(Format::Json, r#"[[[{"a": 1}]]]"#, &limits).codes(),
        ["FLE0003"]
    );
}

#[test]
fn hard_depth_ceiling_applies_even_when_configured_higher() {
    let limits = Limits {
        max_depth: u32::MAX,
        ..Limits::default()
    };
    let n = faultline_doc::HARD_MAX_DEPTH as usize + 1;
    let deep = format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert_eq!(
        parse_with(Format::Json, &deep, &limits).codes(),
        ["FLE0003"]
    );
}

#[test]
fn node_limit_is_enforced() {
    let limits = Limits {
        max_document_nodes: 4,
        ..Limits::default()
    };
    assert!(
        parse_with(Format::Json, "[1, 2, 3]", &limits)
            .parsed
            .document
            .is_some()
    );
    assert_eq!(
        parse_with(Format::Json, "[1, 2, 3, 4]", &limits).codes(),
        ["FLE0004"]
    );
}

#[test]
fn rendered_error_is_readable() {
    let out = json("{\n  \"type\": \"object\",\n}\n");
    let rendered = render::render(out.only_diagnostic(), &out.map);
    assert_eq!(
        rendered,
        "\
error[FLE0101]: trailing comma
 --> <test>:2:19
  |
2 |   \"type\": \"object\",
  |                   ^ JSON does not allow a comma before a closing bracket
  |
  = help: remove this comma
"
    );
}

fn arb_node() -> impl Strategy<Value = Node> {
    let span = Span::empty(SourceId::from_raw(0), 0);
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|i| Value::Number(Decimal::from_i128(i.into()).into())),
        (-1e12f64..1e12)
            .prop_map(|f| Value::Number(Decimal::parse(&format!("{f:?}")).unwrap().into())),
        "\\PC{0,8}".prop_map(Value::String),
        "[\u{0}-\u{1f}\"\\\\]{0,4}".prop_map(Value::String),
    ]
    .prop_map(move |v| Node::new(v, span));
    leaf.prop_recursive(4, 48, 6, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6)
                .prop_map(move |items| Node::new(Value::Array(items), span)),
            prop::collection::vec(("\\PC{0,6}", inner), 0..6).prop_map(move |members| {
                let mut obj = Object::new();
                for (k, v) in members {
                    obj.insert(k, span, v);
                }
                Node::new(Value::Object(obj), span)
            }),
        ]
    })
}

proptest! {
    #[test]
    fn serialized_values_parse_back_to_the_same_value(node in arb_node()) {
        let text = to_json(&node);
        let out = json(&text);
        prop_assert!(out.parsed.diagnostics.is_empty(), "{}", out.rendered());
        prop_assert!(out.root().same_value(&node), "{text}");
    }

    #[test]
    fn arbitrary_input_never_panics_and_spans_stay_in_bounds(text in "\\PC{0,64}|[\\[\\]{}\",:0-9a-z\\\\ -]{0,64}") {
        let out = json(&text);
        for d in &out.parsed.diagnostics {
            for label in &d.labels {
                prop_assert!(label.span.end() as usize <= text.len());
                prop_assert!(text.is_char_boundary(label.span.start() as usize));
                prop_assert!(text.is_char_boundary(label.span.end() as usize));
            }
        }
        if out.parsed.document.is_none() {
            prop_assert!(out.parsed.diagnostics.has_errors());
        }
    }
}
