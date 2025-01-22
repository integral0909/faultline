//! Smithy services as operations.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ParamLocation, Role, StatusKey, TypeKind, validate};
use faultline_rules::{Severity, classify};
use faultline_smithy::lower_smithy;
use faultline_source::SourceMap;

fn api(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///model/weather.smithy", text);
    let sources = SourceMap::new();
    let lowered = lower_smithy(
        &UriRef::parse("file:///model/weather.smithy").unwrap(),
        &loader,
        &sources,
        "weather",
        Role::Both,
    );
    let api = lowered
        .api
        .unwrap_or_else(|| panic!("{:?}", lowered.diagnostics));
    assert_eq!(validate(&api), []);
    api
}

const WEATHER: &str = r#"$version: "2"
namespace example.weather

service Weather {
    version: "2006-03-01"
    resources: [City]
    operations: [GetCurrentTime]
    errors: [Throttled]
}

resource City {
    identifiers: { cityId: CityId }
    read: GetCity
    list: ListCities
    resources: [Forecast]
}

resource Forecast {
    identifiers: { cityId: CityId }
    read: GetForecast
}

string CityId

@readonly
@http(method: "GET", uri: "/cities/{cityId}")
operation GetCity {
    input := for City {
        @required
        @httpLabel
        $cityId

        @httpHeader("X-Units")
        units: String
    }
    output := {
        @required
        name: String

        @httpHeader("ETag")
        etag: String
    }
    errors: [NoSuchResource]
}

@readonly
@http(method: "GET", uri: "/cities?kind=all", code: 200)
operation ListCities {
    input := {
        @httpQuery("pageSize")
        pageSize: Integer

        @httpQuery("nextToken")
        nextToken: String
    }
    output := {
        @required
        items: CityIds
    }
}

list CityIds { member: CityId }

@readonly
@http(method: "GET", uri: "/forecast/{cityId+}")
operation GetForecast {
    input := {
        @required
        @httpLabel
        cityId: CityId
    }
    output := {
        @httpPayload
        chart: Blob
    }
}

@readonly
operation GetCurrentTime {
    output := {
        @required
        time: Timestamp
    }
}

@error("client")
@httpError(404)
structure NoSuchResource {
    @required
    resourceType: String
}

@error("server")
@retryable
structure Throttled {}
"#;

fn operation<'a>(api: &'a Api, key: &str) -> &'a faultline_ir::Operation {
    api.operations
        .iter()
        .find(|o| o.key.to_string() == key)
        .unwrap_or_else(|| {
            panic!(
                "no {key}: {:?}",
                api.operations
                    .iter()
                    .map(|o| o.key.to_string())
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn services_are_operations() {
    let api = api(WEATHER);
    assert_eq!(api.version.as_deref(), Some("2006-03-01"));
    let mut keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "GET /cities",
            "GET /cities/{cityId}",
            "GET /forecast/{cityId}",
            "Weather/GetCurrentTime",
        ],
        "resources, child resources and the service's own operations"
    );

    let get = operation(&api, "GET /cities/{cityId}");
    let parameters: Vec<(ParamLocation, &str, bool)> = get
        .parameters
        .iter()
        .map(|p| (p.location, p.name.as_str(), p.required))
        .collect();
    assert_eq!(
        parameters,
        [
            (ParamLocation::Path, "cityId", true),
            (ParamLocation::Header, "X-Units", false),
        ]
    );
    assert!(get.request_body.is_none(), "every member is bound");
    let statuses: Vec<StatusKey> = get.responses.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [
            StatusKey::Code(200),
            StatusKey::Code(404),
            StatusKey::Code(500)
        ],
        "the service's errors too"
    );
    assert_eq!(get.responses[0].headers[0].name, "ETag");
    let TypeKind::Object(body) = &api.types.get(get.responses[0].content[0].ty).kind else {
        panic!()
    };
    assert_eq!(body.fields.len(), 1, "the header is not in the body");

    let list = operation(&api, "GET /cities");
    let names: Vec<&str> = list.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["pageSize", "nextToken"]);

    let forecast = operation(&api, "GET /forecast/{cityId}");
    assert_eq!(
        forecast.responses[0].content[0].media_type,
        "application/octet-stream"
    );

    let time = operation(&api, "Weather/GetCurrentTime");
    assert!(time.request_body.is_none());
    assert_eq!(time.responses[0].content[0].media_type, "application/json");
}

fn worst(old: &str, new: &str) -> Severity {
    let a = api(old);
    let b = api(new);
    faultline_diff::diff(&a, &b)
        .iter()
        .map(|c| classify("weather", c).severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn operation_changes_are_classified() {
    // A query parameter becomes required.
    assert_eq!(
        worst(
            WEATHER,
            &WEATHER.replace(
                "        @httpQuery(\"pageSize\")",
                "        @required\n        @httpQuery(\"pageSize\")"
            )
        ),
        Severity::Breaking
    );
    // An operation leaves the service.
    assert_eq!(
        worst(WEATHER, &WEATHER.replace("    list: ListCities\n", "")),
        Severity::Breaking
    );
    // A new error clients may not handle.
    let new_error = WEATHER.replace("errors: [NoSuchResource]", "errors: [NoSuchResource, Gone]")
        + "\n@error(\"client\")\n@httpError(410)\nstructure Gone {}\n";
    assert_eq!(worst(WEATHER, &new_error), Severity::Risky);
    // Moving an operation to another path.
    assert_eq!(
        worst(
            WEATHER,
            &WEATHER.replace("uri: \"/cities/{cityId}\"", "uri: \"/city/{cityId}\"")
        ),
        Severity::Breaking
    );
    assert_eq!(worst(WEATHER, WEATHER), Severity::Info);
}
