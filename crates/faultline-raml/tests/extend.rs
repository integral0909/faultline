//! Overlays and extensions merged into the API they extend.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Api;
use faultline_raml::lower_raml;
use faultline_source::{Limits, SourceMap};

const MASTER: &str = "#%RAML 1.0
title: Pets
mediaType: application/json
/pets:
  description: The pets.
  get:
    queryParameters:
      limit?: integer
";

fn lowered(entry: &str, files: &[(&str, &str)]) -> (Option<Api>, Vec<String>) {
    let mut loader = MemoryLoader::new().with("file:///api/pets.raml", MASTER);
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
    let sources = SourceMap::new();
    let lowered = lower_raml(
        &UriRef::parse(entry).unwrap(),
        &loader,
        &sources,
        "t",
        Limits::default(),
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

fn operations(api: &Api) -> Vec<String> {
    api.operations.iter().map(|o| o.key.to_string()).collect()
}

#[test]
fn extensions_add_to_the_api() {
    let extension = "#%RAML 1.0 Extension
extends: ../api/pets.raml
uses:
  lib: lib/types.raml
/pets:
  get:
    queryParameters:
      offset?: integer
  post:
    body: lib.NewPet
";
    let library = "#%RAML 1.0 Library\ntypes:\n  NewPet:\n    properties:\n      name: string\n";
    let (api, codes) = lowered(
        "file:///ext/admin.raml",
        &[
            ("file:///ext/admin.raml", extension),
            ("file:///ext/lib/types.raml", library),
        ],
    );
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert_eq!(operations(&api), ["GET /pets", "POST /pets"]);
    let names: Vec<&str> = api.operations[0]
        .parameters
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["limit", "offset"],
        "maps are merged member by member"
    );
}

#[test]
fn extensions_extend_extensions() {
    let first = "#%RAML 1.0 Extension\nextends: ../api/pets.raml\n/owners:\n  get:\n";
    let second = "#%RAML 1.0 Extension\nextends: first.raml\n/pets:\n  delete:\n";
    let (api, codes) = lowered(
        "file:///ext/second.raml",
        &[
            ("file:///ext/first.raml", first),
            ("file:///ext/second.raml", second),
        ],
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        operations(&api.unwrap()),
        ["GET /pets", "DELETE /pets", "GET /owners"]
    );
}

#[test]
fn overlays_change_only_documentation() {
    let overlay = "#%RAML 1.0 Overlay\nextends: pets.raml\ntitle: Pets (localized)\n/pets:\n  description: Die Haustiere.\n  get:\n    displayName: List\n";
    let (api, codes) = lowered(
        "file:///api/overlay.raml",
        &[("file:///api/overlay.raml", overlay)],
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(operations(&api.unwrap()), ["GET /pets"]);

    let overlay = "#%RAML 1.0 Overlay\nextends: pets.raml\n/pets:\n  post:\n";
    let (api, codes) = lowered(
        "file:///api/overlay.raml",
        &[("file:///api/overlay.raml", overlay)],
    );
    assert!(api.is_none());
    assert_eq!(codes, ["FLE7604"]);
}

#[test]
fn bad_extensions_are_reported() {
    let cases = [
        "#%RAML 1.0 Extension\ntitle: T\n",
        "#%RAML 1.0 Extension\nextends: [pets.raml]\n",
        "#%RAML 1.0 Extension\nextends: lib.raml\n",
        "#%RAML 1.0 Extension\nextends: overlay.raml\n",
    ];
    for extension in cases {
        let (api, codes) = lowered(
            "file:///api/overlay.raml",
            &[
                ("file:///api/overlay.raml", extension),
                ("file:///api/lib.raml", "#%RAML 1.0 Library\n"),
            ],
        );
        assert!(api.is_none(), "{extension}");
        assert!(!codes.is_empty(), "{extension}");
    }
}
