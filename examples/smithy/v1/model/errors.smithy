$version: "2"

namespace example.weather

@error("client")
@httpError(404)
structure NoSuchCity {
    @required
    cityId: CityId
}

@error("server")
@httpError(503)
@retryable
structure Throttled {}
