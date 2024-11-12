//! Loading included files and linking names.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_source::SourceMap;
use faultline_thrift::linker::{DefId, link};
use faultline_thrift::load::load;

/// The codes reported for `files`, the first being the entry, and the
/// loaded files' prefixes.
fn check(files: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
    let sources = SourceMap::new();
    let loaded = load(&UriRef::parse(files[0].0).unwrap(), &loader, &sources);
    let linked = link(&loaded.files);
    let codes = loaded
        .diagnostics
        .iter()
        .chain(linked.diagnostics.iter())
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    let prefixes = loaded.files.iter().map(|f| f.prefix.clone()).collect();
    (codes, prefixes)
}

#[test]
fn includes_are_resolved_and_names_linked() {
    let main = r#"include "common/shared.thrift"
include "types.thrift"
struct Pet { 1: shared.Tag tag, 2: types.Id id, 3: Local local }
struct Local { 1: string s }
service Pets extends shared.Base { void f(1: types.Id id) throws (1: shared.Oops e) }
"#;
    // `types.thrift` is found next to the entry, the including file's
    // directory having no such file.
    let shared = "include \"types.thrift\"\nstruct Tag { 1: types.Id id }\nservice Base {}\nexception Oops {}\n";
    let types = "typedef i64 Id\n";
    let (codes, prefixes) = check(&[
        ("file:///idl/main.thrift", main),
        ("file:///idl/common/shared.thrift", shared),
        ("file:///idl/types.thrift", types),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        prefixes,
        ["main", "shared", "types"],
        "each file is read once"
    );
}

#[test]
fn resolution() {
    let mut loader = MemoryLoader::new();
    loader = loader
        .with("file:///a.thrift", "include \"b.thrift\"\nstruct A {}\n")
        .with("file:///b.thrift", "struct B {}\n");
    let sources = SourceMap::new();
    let loaded = load(
        &UriRef::parse("file:///a.thrift").unwrap(),
        &loader,
        &sources,
    );
    let linked = link(&loaded.files);
    assert_eq!(linked.resolve(0, "A"), Some(DefId { file: 0, index: 0 }));
    assert_eq!(linked.resolve(0, "b.B"), Some(DefId { file: 1, index: 0 }));
    assert_eq!(linked.resolve(0, "B"), None, "included names are qualified");
    assert_eq!(linked.resolve(1, "a.A"), None, "only included files");
}

#[test]
fn problems_are_reported() {
    let cases: &[(&str, &[&str])] = &[
        ("struct A {}\nenum A {}\n", &["FLE8004"]),
        ("struct A { 1: i32 a, 1: i32 b }", &["FLE8005"]),
        ("struct A { 0: i32 a }", &["FLE8005"]),
        ("struct A { 40000: i32 a }", &["FLE8005"]),
        ("struct A { 1: i32 a, 2: i32 a }", &["FLE8005"]),
        ("struct A { i32 a }", &["FLE8008"]),
        ("struct A { 1: Missing m }", &["FLE8003"]),
        ("service S {}\nstruct A { 1: S s }", &["FLE8006"]),
        ("const i32 C = 1\nstruct A { 1: C c }", &["FLE8006"]),
        (
            "struct E {}\nservice S { void f() throws (1: E e) }",
            &["FLE8006"],
        ),
        (
            "exception E {}\ntypedef E T\nservice S { void f() throws (1: T e) }",
            &[],
        ),
        ("service S { oneway i32 f() }", &["FLE8006"]),
        ("typedef B A\ntypedef A B\n", &["FLE8007", "FLE8007"]),
        ("service S extends S {}", &["FLE8007"]),
        (
            "service S extends T {}\nservice T extends S {}",
            &["FLE8007", "FLE8007"],
        ),
        ("struct T {}\nservice S extends T {}", &["FLE8006"]),
        ("service S extends Missing {}", &["FLE8003"]),
        ("include \"missing.thrift\"\n", &["FLE8002"]),
    ];
    for (text, expected) in cases {
        let (mut codes, _) = check(&[("file:///main.thrift", text)]);
        codes.sort();
        assert_eq!(&codes, expected, "{text}");
    }
}

#[test]
fn files_that_include_each_other_are_read_once() {
    let (codes, prefixes) = check(&[
        (
            "file:///a.thrift",
            "include \"b.thrift\"\nstruct A { 1: b.B b }\n",
        ),
        (
            "file:///b.thrift",
            "include \"a.thrift\"\nstruct B { 1: optional a.A a }\n",
        ),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(prefixes, ["a", "b"]);
}
