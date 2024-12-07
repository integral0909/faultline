//! Evaluating queries, with the examples of RFC 9535.

use faultline_doc::{Format, Node};
use faultline_jsonpath::parse;
use faultline_source::{Limits, SourceMap, SourceName};

fn document(json: &str) -> Node {
    let sources = SourceMap::new();
    let file = sources
        .add(SourceName::Virtual("test.json".into()), json.to_owned())
        .unwrap();
    faultline_doc::parse(&file, Format::Json, &Limits::default())
        .document
        .unwrap()
        .root
}

/// The normalized paths a query selects.
fn paths(root: &Node, query: &str) -> Vec<String> {
    parse(query)
        .unwrap_or_else(|e| panic!("{query}: {e}"))
        .select(root)
        .iter()
        .map(|m| m.location.to_string())
        .collect()
}

const STORE: &str = r#"{ "store": {
    "book": [
      { "category": "reference", "author": "Nigel Rees", "title": "Sayings of the Century", "price": 8.95 },
      { "category": "fiction", "author": "Evelyn Waugh", "title": "Sword of Honour", "price": 12.99 },
      { "category": "fiction", "author": "Herman Melville", "title": "Moby Dick", "isbn": "0-553-21311-3", "price": 8.99 },
      { "category": "fiction", "author": "J. R. R. Tolkien", "title": "The Lord of the Rings", "isbn": "0-395-19395-8", "price": 22.99 }
    ],
    "bicycle": { "color": "red", "price": 399 }
  }
}"#;

#[test]
fn the_rfc_examples() {
    let root = document(STORE);
    let book = |i: usize| format!("$['store']['book'][{i}]");
    assert_eq!(
        paths(&root, "$.store.book[*].author"),
        (0..4)
            .map(|i| format!("{}['author']", book(i)))
            .collect::<Vec<_>>()
    );
    assert_eq!(paths(&root, "$..author").len(), 4);
    assert_eq!(
        paths(&root, "$.store.*"),
        ["$['store']['book']", "$['store']['bicycle']"]
    );
    assert_eq!(
        paths(&root, "$.store..price"),
        [
            "$['store']['book'][0]['price']",
            "$['store']['book'][1]['price']",
            "$['store']['book'][2]['price']",
            "$['store']['book'][3]['price']",
            "$['store']['bicycle']['price']",
        ]
    );
    assert_eq!(paths(&root, "$..book[2]"), [book(2)]);
    assert_eq!(
        paths(&root, "$..book[2].author"),
        [format!("{}['author']", book(2))]
    );
    assert!(paths(&root, "$..book[2].publisher").is_empty());
    assert_eq!(paths(&root, "$..book[-1]"), [book(3)]);
    assert_eq!(paths(&root, "$..book[0,1]"), [book(0), book(1)]);
    assert_eq!(paths(&root, "$..book[:2]"), [book(0), book(1)]);
    assert_eq!(paths(&root, "$..book[?@.isbn]"), [book(2), book(3)]);
    assert_eq!(paths(&root, "$..book[?@.price<10]"), [book(0), book(2)]);
    assert_eq!(paths(&root, "$..*").len(), 27);
}

#[test]
fn slices() {
    let root = document(r#"["a", "b", "c", "d", "e", "f", "g"]"#);
    let indices = |query: &str| -> Vec<String> {
        paths(&root, query)
            .into_iter()
            .map(|p| p.trim_start_matches("$[").trim_end_matches(']').to_owned())
            .collect()
    };
    assert_eq!(indices("$[1:3]"), ["1", "2"]);
    assert_eq!(indices("$[5:]"), ["5", "6"]);
    assert_eq!(indices("$[1:5:2]"), ["1", "3"]);
    assert_eq!(indices("$[5:1:-2]"), ["5", "3"]);
    assert_eq!(indices("$[::-1]"), ["6", "5", "4", "3", "2", "1", "0"]);
    assert_eq!(indices("$[-2:]"), ["5", "6"]);
    assert!(indices("$[::0]").is_empty());
    assert!(indices("$[10:20]").is_empty());
}

#[test]
fn filters_and_comparisons() {
    let root = document(
        r#"{"a": [3, 5, 1, 2, 4, 6, {"b": "j"}, {"b": "k"}, {"b": {}}, {"b": "kilo"}, {"b": null}, true, "text"]}"#,
    );
    let count = |query: &str| paths(&root, query).len();
    assert_eq!(count("$.a[?@ > 3.5]"), 3);
    assert_eq!(count("$.a[?@.b == 'kilo']"), 1);
    assert_eq!(count("$.a[?@.b]"), 5, "existence, null included");
    assert_eq!(count("$.a[?@.b == null]"), 1);
    assert_eq!(
        count("$.a[?@.b != 'kilo']"),
        12,
        "a missing value is not 'kilo'"
    );
    assert_eq!(count("$.a[?@.b < 'k']"), 1, "strings compare by code point");
    assert_eq!(count("$.a[?@ == true]"), 1);
    assert_eq!(count("$.a[?@ >= 1 && @ <= 2]"), 2);
    assert_eq!(count("$.a[?@ < 2 || @ > 5]"), 2);
    assert_eq!(count("$.a[?!(@ > 2)]"), 9, "everything not a number over 2");
    assert_eq!(
        count("$.a[?@.missing == @.also_missing]"),
        13,
        "nothing equals nothing"
    );
    assert_eq!(count("$.a[?$.a[0] == 3]"), 13, "absolute queries");
}

#[test]
fn functions() {
    let root = document(
        r#"[{"name": "élan", "tags": ["a", "b"], "date": "1974-05-01"}, {"name": "x", "tags": [], "date": "1974-06-01"}, {"name": "ab\nc"}]"#,
    );
    let count = |query: &str| paths(&root, query).len();
    assert_eq!(count("$[?length(@.name) == 4]"), 2, "characters, not bytes");
    assert_eq!(count("$[?length(@.tags) > 0]"), 1);
    assert_eq!(count("$[?count(@.tags[*]) == 2]"), 1);
    assert_eq!(count("$[?match(@.date, '1974-05-..')]"), 1);
    assert_eq!(count("$[?match(@.date, '1974')]"), 0, "match is anchored");
    assert_eq!(count("$[?search(@.date, '1974')]"), 2);
    assert_eq!(
        count("$[?search(@.name, 'b.c')]"),
        0,
        "`.` excludes line breaks"
    );
    assert_eq!(
        count("$[?match(@.name, '^x$')]"),
        0,
        "`^` and `$` are characters"
    );
    assert_eq!(
        count("$[?match(@.name, '(')]"),
        0,
        "an invalid pattern matches nothing"
    );
    assert_eq!(count("$[?value(@.tags[0]) == 'a']"), 1);
    assert_eq!(
        count("$[?value(@.tags[*]) == 'a']"),
        0,
        "value needs exactly one node"
    );
}

#[test]
fn normalized_paths_escape_names() {
    let root = document(r#"{"it's": {"a\\b": {"\n": 1}}}"#);
    assert_eq!(
        paths(&root, "$..*").last().unwrap(),
        r"$['it\'s']['a\\b']['\n']"
    );
}
