$version: "2"

namespace example.weather

/// Provides weather forecasts.
service Weather {
    version: "2024-06-01"
    resources: [City]
    errors: [Throttled]
}

resource City {
    identifiers: { cityId: CityId }
    properties: { name: String, coordinates: Coordinates }
    read: GetCity
    list: ListCities
}

@pattern("^[a-z0-9-]+$")
string CityId

structure Coordinates {
    @required
    latitude: Double

    @required
    longitude: Double
}

@readonly
@http(method: "GET", uri: "/cities/{cityId}")
operation GetCity {
    input := for City {
        @required
        @httpLabel
        $cityId
    }
    output := for City {
        @required
        $name

        @required
        $coordinates
    }
    errors: [NoSuchCity, CityRetired]
}

@readonly
@http(method: "GET", uri: "/cities")
operation ListCities {
    input := {
        @httpQuery("pageSize")
        @range(min: 1, max: 50)
        pageSize: Integer

        @required
        @httpQuery("nextToken")
        nextToken: String
    }
    output := {
        @required
        items: CitySummaries

        nextToken: String
    }
}

list CitySummaries {
    member: CitySummary
}

structure CitySummary {
    @required
    cityId: CityId

    @required
    name: String

    population: Long
}
