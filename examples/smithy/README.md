# Smithy example

A weather service described in Smithy IDL 2.0, in two versions. Each
version's model is spread over two files: `weather.smithy` has the service,
and `errors.smithy`, beside it, has the errors. Naming either file reads
both.

```sh
faultline diff v1/model/weather.smithy v2/model/weather.smithy
```

| Change | Finding |
|---|---|
| `ListCities`' `nextToken` query parameter is `@required` | breaking: existing requests may lack it (FL2012) |
| `pageSize` is at most 50 | breaking: 100 was accepted and is now rejected (FL1007) |
| `GetCity` can fail with `CityRetired` (410) | risky: clients may not handle the new error (FL2040) |
| `CitySummary` gained `population` | compatible (FL1002) |

The operations have `@http` traits, so they are compared as HTTP
operations (`op[GET /cities/{}]`), with their labels, query parameters,
headers and bodies bound as the traits say. Operations without `@http`
would be compared as remote procedure calls (`op[Weather/GetCity]`).
