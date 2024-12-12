//! Parsing queries: what RFC 9535 accepts and what it refuses.

use faultline_jsonpath::ast::{Comparable, CompareOp, Logical, Selector};
use faultline_jsonpath::parse;

#[test]
fn valid_queries() {
    for query in [
        "$",
        "$.store.book[*].author",
        "$..author",
        "$.store.*",
        "$.store..price",
        "$..book[2]",
        "$..book[-1]",
        "$..book[0,1]",
        "$..book[:2]",
        "$..book[::-1]",
        "$..book[1:5:2]",
        "$[ 1 : 5 : 2 ]",
        "$..book[?@.isbn]",
        "$..book[?@.price<10]",
        "$..book[?(@.price < 10 && @.category == 'fiction')]",
        "$..book[?!@.isbn]",
        "$..book[?!(@.price > 10)]",
        "$..*",
        "$['store']['book']",
        "$[\"a\\\"b\", 'c\\'d']",
        "$['\\u263a', '\\ud83d\\ude00']",
        "$.a .b",
        "$[?length(@.name) > 3]",
        "$[?count(@.*) == 2]",
        "$[?match(@.date, '1974-05-..')]",
        "$[?search(@.name, $.pattern)]",
        "$[?value(@..color) == 'red']",
        "$[?@.a == -0.5e3]",
        "$[?@.a == true || @.b == null]",
        "$.日本",
        "$._x1",
        "$[0]",
        "$[-9007199254740991]",
    ] {
        assert!(parse(query).is_ok(), "{query}: {:?}", parse(query));
    }
}

#[test]
fn invalid_queries() {
    for query in [
        "",
        "@",
        " $",
        "$ ",
        "$.",
        "$. a",
        "$..",
        "$.1a",
        "$[01]",
        "$[-0]",
        "$[9007199254740992]",
        "$['a'",
        "$[a]",
        "$['\\q']",
        "$['\\ud800']",
        "$['\u{1}']",
        "$[?@.a == ]",
        "$[?'a']",
        "$[?1 == 1 ==]",
        "$[?@.* == 1]",
        "$[?@..a == 1]",
        "$[?length(@.name)]",
        "$[?match(@.a)]",
        "$[?count(@.a) == 1 && match(@.b, 'x') == true]",
        "$[?count(1) == 1]",
        "$[?length(@.*) == 1]",
        "$[?unknown(@) == 1]",
        "$[?!@.a == 1]",
        "$[?@.a === 1]",
        "$[?TRUE == @.a]",
        "$[?@.a == 01]",
        "$[?@.a == 1.]",
        "$[1,]",
        "$[?leng\u{508}gth(@) == 1]",
        "$[?fals\u{e9} == 1]",
    ] {
        assert!(
            parse(query).is_err(),
            "{query:?} was accepted: {:?}",
            parse(query)
        );
    }
}

#[test]
fn syntax_trees() {
    let query = parse("$.a['b', 0][?@.c >= 2]").unwrap();
    assert!(!query.relative);
    assert_eq!(query.segments[0].selectors, [Selector::Name("a".into())]);
    assert_eq!(
        query.segments[1].selectors,
        [Selector::Name("b".into()), Selector::Index(0)]
    );
    let Selector::Filter(Logical::Comparison(left, op, right)) = &query.segments[2].selectors[0]
    else {
        panic!("{query:?}")
    };
    assert_eq!(*op, CompareOp::Ge);
    assert!(matches!(**left, Comparable::Query(ref q) if q.relative));
    assert!(matches!(**right, Comparable::Literal(_)));
    let slice = parse("$[1:-1:2]").unwrap();
    assert_eq!(
        slice.segments[0].selectors,
        [Selector::Slice {
            start: Some(1),
            end: Some(-1),
            step: Some(2)
        }]
    );
    assert_eq!(
        parse("$['\\ud83d\\ude00']").unwrap().segments[0].selectors,
        [Selector::Name("😀".into())]
    );
}

#[test]
fn errors_point_at_the_problem() {
    let error = parse("$.a[01]").unwrap_err();
    assert_eq!(error.offset, 4);
    let error = parse("$[?length(@.a)]").unwrap_err();
    assert_eq!(error.offset, 3);
    assert!(error.to_string().contains("must be compared"), "{error}");
}

#[test]
fn deep_nesting_is_refused() {
    let deep = format!("$[?{}@.a{}]", "(".repeat(200), ")".repeat(200));
    assert!(parse(&deep).is_err());
}
