//! Inclusion verdicts on real JSON Schemas. Every counterexample is checked
//! against an independent JSON Schema validator.

mod common;

use common::Schema;
use faultline_subsume::{Checker, Verdict};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    Yes,
    No,
    Unknown,
}

fn verdict(old: &str, new: &str) -> Verdict {
    let a = Schema::new(old);
    let b = Schema::new(new);
    let mut checker = Checker::new(&a.api, &b.api);
    let verdict = checker.subsumes(a.root, b.root);
    if let Verdict::No(w) = &verdict {
        assert!(a.accepts(w), "witness {w} is not accepted by {old}");
        assert!(!b.accepts(w), "witness {w} is accepted by {new}");
    }
    verdict
}

fn expect(cases: &[(&str, &str, Expect)]) {
    for &(old, new, expected) in cases {
        let v = verdict(old, new);
        let got = match v {
            Verdict::Yes => Expect::Yes,
            Verdict::No(_) => Expect::No,
            Verdict::Unknown(_) => Expect::Unknown,
        };
        assert_eq!(got, expected, "{old}  ⊆  {new}  →  {v:?}");
    }
}

#[test]
fn numbers() {
    use Expect::*;
    expect(&[
        (
            r#"{"type":"integer","minimum":0}"#,
            r#"{"type":"integer","minimum":-5}"#,
            Yes,
        ),
        (
            r#"{"type":"integer","minimum":-5}"#,
            r#"{"type":"integer","minimum":0}"#,
            No,
        ),
        (r#"{"type":"integer"}"#, r#"{"type":"number"}"#, Yes),
        (r#"{"type":"number"}"#, r#"{"type":"integer"}"#, No),
        (
            r#"{"type":"number","multipleOf":0.2}"#,
            r#"{"type":"number","multipleOf":0.1}"#,
            Yes,
        ),
        (
            r#"{"type":"number","multipleOf":0.1}"#,
            r#"{"type":"number","multipleOf":0.2}"#,
            No,
        ),
        (
            r#"{"type":"integer","exclusiveMinimum":0.5}"#,
            r#"{"type":"integer","minimum":1}"#,
            Yes,
        ),
        (
            r#"{"type":"number","maximum":10}"#,
            r#"{"type":"number","exclusiveMaximum":10}"#,
            No,
        ),
        (
            r#"{"type":"number","maximum":10}"#,
            r#"{"type":"number","maximum":-1000}"#,
            No,
        ),
        (
            r#"{"type":"integer","multipleOf":2}"#,
            r#"{"type":"integer"}"#,
            Yes,
        ),
        (
            r#"{"type":"number","multipleOf":3}"#,
            r#"{"type":"integer"}"#,
            Yes,
        ),
    ]);
}

#[test]
fn strings() {
    use Expect::*;
    expect(&[
        (
            r#"{"type":"string","maxLength":64}"#,
            r#"{"type":"string","maxLength":32}"#,
            No,
        ),
        (
            r#"{"type":"string","maxLength":32}"#,
            r#"{"type":"string","maxLength":64}"#,
            Yes,
        ),
        (
            r#"{"type":"string"}"#,
            r#"{"type":"string","maxLength":32}"#,
            No,
        ),
        (
            r#"{"type":"string"}"#,
            r#"{"type":"string","minLength":1}"#,
            No,
        ),
        (
            r#"{"type":"string","pattern":"^a"}"#,
            r#"{"type":"string","pattern":"^a"}"#,
            Yes,
        ),
        (
            r#"{"type":"string","format":"email"}"#,
            r#"{"type":"string"}"#,
            Yes,
        ),
        // Patterns are compared as regular languages.
        (
            r#"{"type":"string"}"#,
            r#"{"type":"string","pattern":"^a"}"#,
            No,
        ),
        (
            r#"{"type":"string","pattern":"^[a-z]{2,5}$"}"#,
            r#"{"type":"string","pattern":"^[a-z]+$"}"#,
            Yes,
        ),
        (
            r#"{"type":"string","pattern":"^[a-z]+$"}"#,
            r#"{"type":"string","pattern":"^[a-z]{2,5}$"}"#,
            No,
        ),
        (
            r#"{"type":"string","pattern":"^[A-Z]{3}-\\d+$"}"#,
            r#"{"type":"string","pattern":"-\\d"}"#,
            Yes,
        ),
        // A difference outside the left lengths is not a witness.
        (
            r#"{"type":"string","pattern":"^a*$","maxLength":3}"#,
            r#"{"type":"string","pattern":"^a{0,3}$"}"#,
            Yes,
        ),
        (
            r#"{"type":"string","pattern":"^a*$","minLength":2}"#,
            r#"{"type":"string","pattern":"^a{2,}$"}"#,
            Yes,
        ),
        // Constructs outside regular languages stay undecided.
        (
            r#"{"type":"string"}"#,
            r#"{"type":"string","pattern":"^(a)\\1$"}"#,
            Unknown,
        ),
    ]);
}

#[test]
fn enums_and_constants() {
    use Expect::*;
    expect(&[
        (r#"{"enum":["a","b"]}"#, r#"{"enum":["a","b","c"]}"#, Yes),
        (r#"{"enum":["a","b","c"]}"#, r#"{"enum":["a","b"]}"#, No),
        (r#"{"enum":["a"]}"#, r#"{"type":"string"}"#, Yes),
        (r#"{"type":"string"}"#, r#"{"enum":["a"]}"#, No),
        (r#"{"const":1}"#, r#"{"type":"integer","minimum":1}"#, Yes),
        (r#"{"enum":[1, 2.5]}"#, r#"{"type":"integer"}"#, No),
    ]);
}

#[test]
fn objects() {
    use Expect::*;
    let ab = r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a"]}"#;
    expect(&[
        (
            ab,
            r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a","b"]}"#,
            No,
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"#,
            r#"{"type":"object","properties":{"a":{"type":"string"}}}"#,
            Yes,
        ),
        (
            ab,
            r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}},"required":["a"],"additionalProperties":false}"#,
            No,
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}"#,
            r#"{"type":"object","properties":{"a":{"type":"string"}}}"#,
            Yes,
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}}}"#,
            r#"{"type":"object","properties":{"a":{"type":"string","maxLength":3}}}"#,
            No,
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"additionalProperties":false}"#,
            r#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}"#,
            No,
        ),
        (
            r#"{"type":"object","minProperties":2}"#,
            r#"{"type":"object","minProperties":1}"#,
            Yes,
        ),
        (
            r#"{"type":"object"}"#,
            r#"{"type":"object","maxProperties":1}"#,
            No,
        ),
    ]);
}

#[test]
fn arrays() {
    use Expect::*;
    expect(&[
        (
            r#"{"type":"array","items":{"type":"integer"}}"#,
            r#"{"type":"array","items":{"type":"number"}}"#,
            Yes,
        ),
        (
            r#"{"type":"array","items":{"type":"number"}}"#,
            r#"{"type":"array","items":{"type":"integer"}}"#,
            No,
        ),
        (
            r#"{"type":"array","minItems":1}"#,
            r#"{"type":"array","minItems":2}"#,
            No,
        ),
        (
            r#"{"type":"array","items":{"type":"integer"}}"#,
            r#"{"type":"array","items":{"type":"integer"},"uniqueItems":true}"#,
            No,
        ),
        (
            r#"{"type":"array","items":{"type":"integer"},"uniqueItems":true}"#,
            r#"{"type":"array","items":{"type":"integer"}}"#,
            Yes,
        ),
        (
            r#"{"type":"array","maxItems":3}"#,
            r#"{"type":"array","maxItems":5}"#,
            Yes,
        ),
        // Longer arrays than the item sampler has distinct values for are
        // still found, by repeating items.
        (
            r#"{"type":"array","items":{"type":"string"},"maxItems":10}"#,
            r#"{"type":"array","items":{"type":"string"},"maxItems":5}"#,
            No,
        ),
        (
            r#"{"type":"array","items":{"type":"boolean"},"maxItems":10}"#,
            r#"{"type":"array","items":{"type":"boolean"},"maxItems":5}"#,
            No,
        ),
        (
            r#"{"type":"array","prefixItems":[{"type":"string"}],"items":false}"#,
            r#"{"type":"array","prefixItems":[{"type":"string","minLength":1}]}"#,
            No,
        ),
    ]);
}

#[test]
fn unions_and_nullability() {
    use Expect::*;
    expect(&[
        (r#"{"type":"string"}"#, r#"{"type":["string","null"]}"#, Yes),
        (r#"{"type":["string","null"]}"#, r#"{"type":"string"}"#, No),
        (
            r#"{"anyOf":[{"type":"string"},{"type":"integer"}]}"#,
            r#"{"anyOf":[{"type":"integer"},{"type":"string"}]}"#,
            Yes,
        ),
        (r#"{"type":"string"}"#, r#"{"not":{"type":"null"}}"#, Yes),
        (
            r#"{"type":"integer","minimum":5,"maximum":4}"#,
            r#"{"const":"x"}"#,
            Yes,
        ),
    ]);
}

#[test]
fn adding_an_overlapping_one_of_variant_rejects_values() {
    // Integers now match both "integer" and "number", so they are rejected.
    let v = verdict(
        r#"{"oneOf":[{"type":"integer"},{"type":"string"}]}"#,
        r#"{"oneOf":[{"type":"integer"},{"type":"string"},{"type":"number"}]}"#,
    );
    let Verdict::No(w) = v else { panic!("{v:?}") };
    assert!(matches!(w, faultline_ir::Literal::Number(_)), "{w}");
}

#[test]
fn recursive_types() {
    use Expect::*;
    let tree = |name: &str| {
        format!(
            r##"{{"type":"object","required":["name"],"properties":{{"name":{name},"children":{{"type":"array","items":{{"$ref":"#"}}}}}}}}"##
        )
    };
    let plain = tree(r#"{"type":"string"}"#);
    let short = tree(r#"{"type":"string","maxLength":3}"#);
    expect(&[
        (&plain, &plain, Yes),
        (&short, &plain, Yes),
        (&plain, &short, No),
    ]);
}

#[test]
fn composition_with_all_of() {
    use Expect::*;
    let composed = r##"{"allOf":[{"$ref":"#/$defs/Base"},{"type":"object","properties":{"b":{"type":"string"}}}],"$defs":{"Base":{"type":"object","properties":{"a":{"type":"integer"}},"required":["a"]}}}"##;
    let narrowed = r##"{"allOf":[{"$ref":"#/$defs/Base"},{"type":"object","properties":{"b":{"type":"string","maxLength":1}}}],"$defs":{"Base":{"type":"object","properties":{"a":{"type":"integer"}},"required":["a"]}}}"##;
    expect(&[
        (composed, composed, Yes),
        (narrowed, composed, Yes),
        (composed, narrowed, No),
    ]);
}

#[test]
fn pattern_properties() {
    use Expect::*;
    expect(&[
        (
            r#"{"type":"object","patternProperties":{"^x-":{"type":"string"}}}"#,
            r#"{"type":"object","patternProperties":{"^x-":{"type":"string"}}}"#,
            Yes,
        ),
        (
            r#"{"type":"object","patternProperties":{"^x-":{"type":"string"}}}"#,
            r#"{"type":"object","patternProperties":{"^x-":{"type":"integer"}}}"#,
            No,
        ),
        (
            r#"{"type":"object","patternProperties":{"^x-":{"type":"string"}},"additionalProperties":false}"#,
            r#"{"type":"object","patternProperties":{"^x-":{}},"additionalProperties":false}"#,
            Yes,
        ),
    ]);
}
