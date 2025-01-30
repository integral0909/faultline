//! Models in the JSON AST form are the models the IDL describes.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Role, validate};
use faultline_smithy::lower_smithy;
use faultline_source::SourceMap;

fn lowered(uri: &str, text: &str) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new().with(uri, text);
    let sources = SourceMap::new();
    let lowered = lower_smithy(
        &UriRef::parse(uri).unwrap(),
        &loader,
        &sources,
        "w",
        Role::Both,
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

const IDL: &str = r#"$version: "2"
namespace example.weather

service Weather {
    version: "1"
    resources: [City]
    errors: [Throttled]
}

resource City {
    identifiers: { cityId: CityId }
    read: GetCity
}

@pattern("^[a-z]+$")
string CityId

@readonly
@http(method: "GET", uri: "/cities/{cityId}")
operation GetCity {
    input: GetCityInput
    output: GetCityOutput
    errors: [NoSuchCity]
}

@input
structure GetCityInput {
    @required
    @httpLabel
    cityId: CityId
}

@output
structure GetCityOutput with [Named] {
    kind: Kind
    tags: Tags
}

@mixin
structure Named {
    @required
    name: String
}

enum Kind {
    BIG = "big"
    SMALL = "small"
}

list Tags { member: String }

@error("client")
@httpError(404)
structure NoSuchCity {}

@error("server")
structure Throttled {}
"#;

const JSON: &str = r#"{
  "smithy": "2.0",
  "shapes": {
    "example.weather#Weather": {
      "type": "service",
      "version": "1",
      "resources": [{"target": "example.weather#City"}],
      "errors": [{"target": "example.weather#Throttled"}]
    },
    "example.weather#City": {
      "type": "resource",
      "identifiers": {"cityId": {"target": "example.weather#CityId"}},
      "read": {"target": "example.weather#GetCity"}
    },
    "example.weather#CityId": {
      "type": "string",
      "traits": {"smithy.api#pattern": "^[a-z]+$"}
    },
    "example.weather#GetCity": {
      "type": "operation",
      "input": {"target": "example.weather#GetCityInput"},
      "output": {"target": "example.weather#GetCityOutput"},
      "errors": [{"target": "example.weather#NoSuchCity"}],
      "traits": {
        "smithy.api#readonly": {},
        "smithy.api#http": {"method": "GET", "uri": "/cities/{cityId}"}
      }
    },
    "example.weather#GetCityInput": {
      "type": "structure",
      "members": {
        "cityId": {
          "target": "example.weather#CityId",
          "traits": {"smithy.api#required": {}, "smithy.api#httpLabel": {}}
        }
      },
      "traits": {"smithy.api#input": {}}
    },
    "example.weather#GetCityOutput": {
      "type": "structure",
      "mixins": [{"target": "example.weather#Named"}],
      "members": {
        "kind": {"target": "example.weather#Kind"},
        "tags": {"target": "example.weather#Tags"}
      },
      "traits": {"smithy.api#output": {}}
    },
    "example.weather#Named": {
      "type": "structure",
      "members": {
        "name": {"target": "smithy.api#String", "traits": {"smithy.api#required": {}}}
      },
      "traits": {"smithy.api#mixin": {}}
    },
    "example.weather#Kind": {
      "type": "enum",
      "members": {
        "BIG": {"target": "smithy.api#Unit", "traits": {"smithy.api#enumValue": "big"}},
        "SMALL": {"target": "smithy.api#Unit", "traits": {"smithy.api#enumValue": "small"}}
      }
    },
    "example.weather#Tags": {
      "type": "list",
      "member": {"target": "smithy.api#String"}
    },
    "example.weather#NoSuchCity": {
      "type": "structure",
      "members": {},
      "traits": {"smithy.api#error": "client", "smithy.api#httpError": 404}
    },
    "example.weather#Throttled": {
      "type": "structure",
      "members": {},
      "traits": {"smithy.api#error": "server"}
    }
  }
}"#;

#[test]
fn the_json_ast_is_the_idl_model() {
    let (idl, codes) = lowered("file:///model/weather.smithy", IDL);
    assert!(codes.is_empty(), "{codes:?}");
    let (json, codes) = lowered("file:///build/model.json", JSON);
    assert!(codes.is_empty(), "{codes:?}");
    let (idl, json) = (idl.unwrap(), json.unwrap());
    assert_eq!(validate(&json), []);
    assert_eq!(json.operations.len(), 1);
    let changes = faultline_diff::diff(&idl, &json);
    assert!(changes.is_empty(), "{:?}", changes.changes);
}

#[test]
fn applied_traits_and_problems() {
    let applied = r#"{"smithy": "2.0", "shapes": {
        "a#S": {"type": "structure", "members": {"m": {"target": "smithy.api#String"}}},
        "a#S$m": {"type": "apply", "traits": {"smithy.api#required": {}}}
    }}"#;
    let (api, codes) = lowered("file:///m.json", applied);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert_eq!(
        fields_of(&api),
        [("m".to_owned(), true)],
        "required through apply"
    );

    for (text, code) in [
        (r#"{"shapes": {}}"#, "FLE8401"),
        (
            r#"{"smithy": "2.0", "shapes": {"NoNamespace": {"type": "string"}}}"#,
            "FLE8401",
        ),
        (
            r#"{"smithy": "2.0", "shapes": {"a#S": {"type": "thing"}}}"#,
            "FLE8401",
        ),
        (
            r#"{"smithy": "2.0", "shapes": {"a#S": {"type": "structure", "members": {"m": {"target": "a#Missing"}}}}}"#,
            "FLE8402",
        ),
    ] {
        let (_, codes) = lowered("file:///m.json", text);
        assert!(codes.iter().any(|c| c == code), "{text}: {codes:?}");
    }
}

/// The first root's fields and whether they are required.
fn fields_of(api: &Api) -> Vec<(String, bool)> {
    let ty = api.types.resolve(api.schemas[0].ty);
    let faultline_ir::TypeKind::Object(object) = &api.types.get(ty).kind else {
        panic!()
    };
    object
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.required))
        .collect()
}
