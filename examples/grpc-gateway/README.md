# gRPC service against its OpenAPI description

A gRPC service exposed over HTTP/JSON through `google.api.http` rules
(gRPC-Gateway, Envoy's transcoder, Cloud Endpoints), and the OpenAPI
description published for its JSON clients. The two have drifted apart.

```sh
faultline diff openapi.yaml proto/pets.proto --new-format protobuf-http
```

`--new-format protobuf-http` reads the `.proto` file as the HTTP/JSON API its
rules expose. With the description as the old version and the service as
the new one, every breaking finding is a promise the description makes and
the service does not keep:

| Finding | Cause |
|---|---|
| `DELETE /v1/{name}` removed (FL2001) | documented, but no RPC is bound to it |
| `weightGrams` changed from string to integer (FL1001) | the field became `int32`, which JSON writes as a number; `int64` is written as a string |
| query parameter `pageSize` removed (FL2011) | `page_size` was renamed `max_page_size`, so the documented parameter is ignored |
| query parameter `maxPageSize` added | compatible, but undocumented |

Run it in CI next to the service's build to keep the published description
honest.
