//! Parsing Smithy IDL.

use faultline_smithy::ast::{IoShape, Model, NodeValue, ShapeKind, SimpleType};
use faultline_smithy::parser::parse;
use faultline_source::SourceId;

fn parsed(text: &str) -> (Model, Vec<String>) {
    let (model, diagnostics) = parse(SourceId::from_raw(0), text);
    let messages = diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}: {}",
                d.code.map(|c| c.to_string()).unwrap_or_default(),
                d.message
            )
        })
        .collect();
    (model, messages)
}

const WEATHER: &str = r#"$version: "2"
metadata validators = [{name: "EmitEachSelector"}]

namespace example.weather

use aws.protocols#restJson1

/// Provides weather forecasts.
@restJson1
@title("Weather Service")
service Weather {
    version: "2006-03-01"
    resources: [City]
    operations: [GetCurrentTime]
}

resource City {
    identifiers: { cityId: CityId }
    read: GetCity
}

@pattern("^[A-Za-z0-9 ]+$")
string CityId

@readonly
@http(method: "GET", uri: "/cities/{cityId}", code: 200)
operation GetCity {
    input := for City {
        @required
        @httpLabel
        $cityId
    }
    output := {
        @required
        name: String

        @required
        coordinates: CityCoordinates
    }
    errors: [NoSuchResource]
}

structure CityCoordinates {
    @required
    latitude: Float
    @required
    longitude: Float = 0
}

@error("client")
@httpError(404)
structure NoSuchResource {
    @required
    resourceType: String
}

@readonly
operation GetCurrentTime {
    output: GetCurrentTimeOutput
}

@output
structure GetCurrentTimeOutput with [Timed] {
    @required
    time: Timestamp
}

@mixin
structure Timed {}

enum Kind {
    CITY = "city"
    /// A town.
    TOWN
}

intEnum Level {
    LOW = 1
    HIGH = 2
}

list Cities {
    member: CityId
}

map Populations {
    key: CityId
    value: Long
}

union Place {
    city: City
    point: CityCoordinates
}

apply CityId @documentation("A city's identifier.")
apply NoSuchResource {
    @deprecated
    @sensitive
}
"#;

#[test]
fn a_complete_model() {
    let (model, messages) = parsed(WEATHER);
    assert!(messages.is_empty(), "{messages:?}");
    assert_eq!(model.version.as_deref(), Some("2"));
    assert_eq!(model.namespace.as_ref().unwrap().text, "example.weather");
    assert_eq!(model.uses[0].text, "aws.protocols#restJson1");
    let names: Vec<(&str, ShapeKind)> = model
        .shapes
        .iter()
        .map(|s| (s.name.text.as_str(), s.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("Weather", ShapeKind::Service),
            ("City", ShapeKind::Resource),
            ("CityId", ShapeKind::Simple(SimpleType::String)),
            ("GetCity", ShapeKind::Operation),
            ("CityCoordinates", ShapeKind::Structure),
            ("NoSuchResource", ShapeKind::Structure),
            ("GetCurrentTime", ShapeKind::Operation),
            ("GetCurrentTimeOutput", ShapeKind::Structure),
            ("Timed", ShapeKind::Structure),
            ("Kind", ShapeKind::Enum),
            ("Level", ShapeKind::IntEnum),
            ("Cities", ShapeKind::List),
            ("Populations", ShapeKind::Map),
            ("Place", ShapeKind::Union),
        ]
    );
    let weather = &model.shapes[0];
    assert_eq!(weather.doc.as_deref(), Some("Provides weather forecasts."));
    assert_eq!(weather.traits[1].value.as_str(), Some("Weather Service"));
    let operations = weather
        .properties
        .as_ref()
        .unwrap()
        .get("operations")
        .unwrap();
    assert!(
        matches!(&operations.value, NodeValue::Array(items) if items[0].as_str() == Some("GetCurrentTime"))
    );

    let get_city = &model.shapes[3];
    let http = &get_city.traits[1];
    assert_eq!(http.id.text, "http");
    assert_eq!(
        http.value.get("uri").unwrap().as_str(),
        Some("/cities/{cityId}")
    );
    let body = get_city.operation.as_ref().unwrap();
    let Some(IoShape::Inline(input)) = &body.input else {
        panic!()
    };
    assert_eq!(input.name.text, "GetCityInput");
    assert_eq!(input.for_resource.as_ref().unwrap().text, "City");
    assert!(input.members[0].target.is_none(), "elided");
    assert_eq!(input.members[0].traits.len(), 2);
    assert_eq!(body.errors[0].text, "NoSuchResource");

    let coordinates = &model.shapes[4];
    assert!(matches!(
        coordinates.members[1].default.as_ref().unwrap().value,
        NodeValue::Number(ref n) if n == "0"
    ));
    let output = &model.shapes[7];
    assert_eq!(output.mixins[0].text, "Timed");
    let kind = &model.shapes[9];
    assert_eq!(kind.members[1].doc.as_deref(), Some("A town."));
    assert_eq!(model.applies.len(), 2);
    assert_eq!(model.applies[1].traits.len(), 2);
}

#[test]
fn syntax_errors_are_reported_and_parsing_resumes() {
    let (model, messages) = parsed(
        "namespace a\nstructure A { b: }\nstructure B { c: String }\noperation C { inputs: D }\nstring E",
    );
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages.iter().all(|m| m.starts_with("FLE8401")));
    let names: Vec<&str> = model.shapes.iter().map(|s| s.name.text.as_str()).collect();
    assert_eq!(names, ["B", "E"]);
}

#[test]
fn deep_values_are_refused() {
    let deep = format!(
        "namespace a\n@t({}1{})\nstring S",
        "[".repeat(100),
        "]".repeat(100)
    );
    let (_, messages) = parsed(&deep);
    assert!(!messages.is_empty());
}
