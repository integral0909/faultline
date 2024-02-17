mod common;

use std::fmt::Write as _;

use common::{json, parse_with, to_json, yaml};
use faultline_doc::{Format, Kind, Node, Number, Value};
use faultline_source::Limits;
use proptest::prelude::*;

fn scalar(text: &str) -> Node {
    let out = yaml(&format!("v: {text}\n"));
    out.assert_clean();
    out.root().get("v").unwrap().clone()
}

#[test]
fn plain_scalars_follow_the_core_schema() {
    let number = |s: &str| Some(s.to_owned());
    let cases: &[(&str, Kind, Option<String>)] = &[
        ("~", Kind::Null, None),
        ("null", Kind::Null, None),
        ("NULL", Kind::Null, None),
        ("true", Kind::Bool, None),
        ("False", Kind::Bool, None),
        ("12", Kind::Number, number("12")),
        ("+12", Kind::Number, number("12")),
        ("-0", Kind::Number, number("0")),
        ("0o17", Kind::Number, number("15")),
        ("0x1F", Kind::Number, number("31")),
        ("1.5", Kind::Number, number("1.5")),
        (".5", Kind::Number, number("0.5")),
        ("1e3", Kind::Number, number("1000")),
        (".inf", Kind::Number, number(".inf")),
        ("-.Inf", Kind::Number, number("-.inf")),
        (".NaN", Kind::Number, number(".nan")),
        // YAML 1.1 booleans and number spellings are plain strings in 1.2.
        ("yes", Kind::String, None),
        ("NO", Kind::String, None),
        ("on", Kind::String, None),
        ("1_000", Kind::String, None),
        ("0b101", Kind::String, None),
        ("-.nan", Kind::String, None),
        ("0x", Kind::String, None),
        ("1.2.3", Kind::String, None),
        ("2001-12-14", Kind::String, None),
    ];
    for (text, kind, rendered) in cases {
        let node = scalar(text);
        assert_eq!(node.kind(), *kind, "{text}");
        if let Some(expected) = rendered {
            assert_eq!(&node.as_number().unwrap().to_string(), expected, "{text}");
        }
        if *kind == Kind::String {
            assert_eq!(node.as_str(), Some(*text));
        }
    }
}

#[test]
fn quoted_and_block_scalars_are_strings() {
    let out = yaml("a: '12'\nb: \"true\"\nc: |\n  line one\n  line two\nd: >-\n  folded\n  text\n");
    out.assert_clean();
    let root = out.root();
    assert_eq!(root.get("a").unwrap().as_str(), Some("12"));
    assert_eq!(root.get("b").unwrap().as_str(), Some("true"));
    assert_eq!(
        root.get("c").unwrap().as_str(),
        Some("line one\nline two\n")
    );
    assert_eq!(root.get("d").unwrap().as_str(), Some("folded text"));
}

#[test]
fn explicit_core_tags() {
    let out = yaml(
        "a: !!str 123\nb: !!int \"42\"\nc: !!float 1\nd: !!bool 'true'\ne: !!null ''\nf: ! 12\n",
    );
    out.assert_clean();
    let root = out.root();
    assert_eq!(root.get("a").unwrap().as_str(), Some("123"));
    assert_eq!(
        root.get("b").unwrap().as_number().unwrap().to_string(),
        "42"
    );
    assert_eq!(root.get("c").unwrap().kind(), Kind::Number);
    assert_eq!(root.get("d").unwrap().as_bool(), Some(true));
    assert!(root.get("e").unwrap().is_null());
    assert_eq!(root.get("f").unwrap().as_str(), Some("12"));
}

#[test]
fn verbatim_core_tags_match_their_shorthand() {
    let out = yaml(
        "a: !<tag:yaml.org,2002:str> 1
b: !<tag:yaml.org,2002:map> {x: 1}
",
    );
    out.assert_clean();
    assert_eq!(out.root().get("a").unwrap().as_str(), Some("1"));
    let out = yaml(
        "a: !<tag:example.com,2026:thing> 1
",
    );
    assert_eq!(out.codes(), ["FLE0205"]);
    assert!(
        out.parsed
            .diagnostics
            .iter()
            .next()
            .unwrap()
            .message
            .contains("!<tag:example.com,2026:thing>")
    );
}

#[test]
fn tag_mismatches_and_unknown_tags() {
    let out = yaml("a: !!int 1.5\nb: !!bool yes\n");
    assert_eq!(out.codes(), ["FLE0206", "FLE0206"]);
    assert_eq!(out.root().get("a").unwrap().as_str(), Some("1.5"));

    let out = yaml("a: !custom 12\nb: !!binary aGk=\nc: !!set {x: null}\n");
    assert_eq!(out.codes(), ["FLE0205", "FLE0205", "FLE0205"]);
    assert!(!out.parsed.diagnostics.has_errors());
    assert_eq!(out.root().get("a").unwrap().kind(), Kind::Number);
    assert_eq!(out.root().get("b").unwrap().as_str(), Some("aGk="));
}

#[test]
fn keys_are_strings_even_when_they_look_like_other_types() {
    let out = yaml("responses:\n  200: ok\n  default: err\ntrue: 1\n~: 2\n1.50: 3\n'quoted': 4\n");
    out.assert_clean();
    let root = out.root();
    let responses = root.get("responses").unwrap().as_object().unwrap();
    assert_eq!(responses.keys().collect::<Vec<_>>(), ["200", "default"]);
    assert_eq!(
        root.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["responses", "true", "~", "1.50", "quoted"]
    );
}

#[test]
fn collection_keys_are_rejected() {
    let out = yaml("? [a, b]\n: value\n");
    assert_eq!(out.codes(), ["FLE0204"]);
    assert_eq!(out.primary_text(out.only_diagnostic()), "[a, b]");
}

#[test]
fn spans_are_byte_offsets_even_with_non_ascii_text() {
    let text = "名前: \"値\"\nlist: [α, β]\nblock:\n  x: 1\n  y: [2]\n";
    let out = yaml(text);
    out.assert_clean();
    let root = out.root();
    let obj = root.as_object().unwrap();
    assert_eq!(out.slice(obj.member("名前").unwrap().key_span), "名前");
    assert_eq!(out.slice(root.get("名前").unwrap().span), "\"値\"");
    assert_eq!(out.slice(root.get("list").unwrap().span), "[α, β]");
    let beta = &root.get("list").unwrap().as_array().unwrap()[1];
    assert_eq!(out.slice(beta.span), "β");
    assert_eq!(out.slice(root.get("block").unwrap().span), "x: 1\n  y: [2]");
    assert_eq!(out.slice(root.span), text.trim_end());
}

#[test]
fn aliases_copy_the_anchored_value_with_its_original_spans() {
    let out = yaml("base: &pet {name: string}\nother: *pet\nlist: [&n 1, *n]\n");
    out.assert_clean();
    let root = out.root();
    let base = root.get("base").unwrap();
    let other = root.get("other").unwrap();
    assert!(base.same_value(other));
    assert_eq!(other.span, base.span);
    let list = root.get("list").unwrap().as_array().unwrap();
    assert!(list[0].same_value(&list[1]));
}

#[test]
fn aliases_can_be_used_as_keys_when_they_name_scalars() {
    let out = yaml("k: &key name\nm:\n  *key : value\n");
    out.assert_clean();
    assert_eq!(
        out.root().get("m").unwrap().get("name").unwrap().as_str(),
        Some("value")
    );
}

#[test]
fn merge_keys_fill_in_missing_members() {
    let out = yaml(
        "a: &a {x: 1, y: 1}\nb: &b {y: 2, z: 2}\n\
         single:\n  <<: *a\n  y: 9\n\
         many:\n  <<: [*a, *b]\n\
         quoted:\n  \"<<\": literal\n",
    );
    out.assert_clean();
    let root = out.root();
    let get = |map: &str, key: &str| {
        root.get(map)
            .unwrap()
            .get(key)
            .map(to_json)
            .unwrap_or_default()
    };
    // Explicit keys win over merged ones.
    assert_eq!(get("single", "x"), "1");
    assert_eq!(get("single", "y"), "9");
    // Earlier merge sources win over later ones.
    assert_eq!(get("many", "y"), "1");
    assert_eq!(get("many", "z"), "2");
    assert!(root.get("many").unwrap().get("<<").is_none());
    // A quoted "<<" is an ordinary key.
    assert_eq!(get("quoted", "<<"), "\"literal\"");
}

#[test]
fn invalid_merge_values_are_reported() {
    let out = yaml("m:\n  <<: 3\n  a: 1\n");
    assert_eq!(out.codes(), ["FLE0207"]);
    assert_eq!(
        out.root().get("m").unwrap().get("a").unwrap().kind(),
        Kind::Number
    );
}

#[test]
fn duplicate_keys_are_reported() {
    let out = yaml("a: 1\nb: 2\na: 3\n");
    assert_eq!(out.codes(), ["FLE0005"]);
    let d = out.only_diagnostic();
    assert_eq!(out.primary_text(d), "a");
    assert_eq!(d.labels[1].span.start(), 0);
}

#[test]
fn stream_level_errors() {
    assert_eq!(yaml("a: 1\n---\nb: 2\n").codes(), ["FLE0203"]);
    assert_eq!(yaml("a: *missing\n").codes(), ["FLE0202"]);
    assert_eq!(yaml("a: [1, 2\n").codes(), ["FLE0201"]);
    assert_eq!(yaml("a: b: c\n").codes(), ["FLE0201"]);
}

#[test]
fn empty_and_comment_only_documents_are_null() {
    for text in ["", "# nothing here\n", "---\n", "---\n...\n"] {
        let out = yaml(text);
        out.assert_clean();
        assert!(out.root().is_null(), "{text:?}");
    }
}

#[test]
fn alias_expansion_is_bounded_by_the_node_limit() {
    // Each level multiplies the size by ten: ten levels would be 10^10 nodes.
    let mut text = String::from("l0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..10 {
        let prev = format!("*l{}", level - 1);
        let items = [prev.as_str(); 10].join(", ");
        let _ = writeln!(text, "l{level}: &l{level} [{items}]");
    }
    let limits = Limits {
        max_document_nodes: 100_000,
        ..Limits::default()
    };
    let out = parse_with(Format::Yaml, &text, &limits);
    assert_eq!(out.codes(), ["FLE0004"]);
    assert!(out.parsed.document.is_none());
    // The error points at the alias whose expansion crossed the limit.
    assert_eq!(out.primary_text(out.only_diagnostic()), "*l3");
}

#[test]
fn alias_expansion_respects_the_depth_limit() {
    let limits = Limits {
        max_depth: 4,
        ..Limits::default()
    };
    let ok = parse_with(Format::Yaml, "a: &a [[1]]\nb: [*a]\n", &limits);
    ok.assert_clean();
    let too_deep = parse_with(Format::Yaml, "a: &a [[[1]]]\nb: [[*a]]\n", &limits);
    assert_eq!(too_deep.codes(), ["FLE0003"]);
}

#[test]
fn nesting_limit_applies_to_plain_collections() {
    let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
    assert_eq!(yaml(&deep).codes(), ["FLE0003"]);
}

fn arb_json_text() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        any::<i32>().prop_map(|i| i.to_string()),
        "[a-zA-Z0-9 ]{0,8}".prop_map(|s| format!("\"{s}\"")),
    ];
    leaf.prop_recursive(3, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(|v| format!("[{}]", v.join(", "))),
            prop::collection::btree_map("[a-z]{1,6}", inner, 0..5).prop_map(|m| {
                let members: Vec<String> = m
                    .into_iter()
                    .map(|(k, v)| format!("\"{k}\": {v}"))
                    .collect();
                format!("{{{}}}", members.join(", "))
            }),
        ]
    })
}

proptest! {
    #[test]
    fn json_text_means_the_same_as_yaml(text in arb_json_text()) {
        let as_json = json(&text);
        let as_yaml = yaml(&text);
        as_json.assert_clean();
        as_yaml.assert_clean();
        prop_assert!(as_json.root().same_value(as_yaml.root()), "{text}");
    }

    #[test]
    fn arbitrary_input_never_panics(text in "\\PC{0,48}|[-:?&*!|>'\"\\[\\]{},#\n a-z0-9]{0,48}") {
        let out = yaml(&text);
        for d in &out.parsed.diagnostics {
            for label in &d.labels {
                prop_assert!(label.span.end() as usize <= text.len());
                prop_assert!(text.is_char_boundary(label.span.start() as usize));
            }
        }
        if out.parsed.document.is_none() {
            prop_assert!(out.parsed.diagnostics.has_errors());
        }
    }
}

#[test]
fn special_floats_are_preserved() {
    let out = yaml("[.inf, -.inf, .nan]");
    let items = out.root().as_array().unwrap();
    assert_eq!(items[0].value, Value::Number(Number::PosInfinity));
    assert_eq!(items[1].value, Value::Number(Number::NegInfinity));
    assert_eq!(items[2].value, Value::Number(Number::NaN));
}
