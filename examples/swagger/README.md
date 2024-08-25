# Swagger 2.0 example

The pet store's first version described in Swagger 2.0 (`swagger.yaml`) and
the same API migrated to OpenAPI 3.0 (`openapi.yaml`, a copy of
`../petstore/v1/openapi.yaml`).

```sh
faultline diff swagger.yaml openapi.yaml
```

The migration changes nothing a client can observe, so there are no
findings. Swagger 2.0 is read as the official converters translate it:
`body` parameters and `consumes` become request bodies, response `schema`s
and `produces` become response content, `definitions` become schemas, and
`collectionFormat` becomes `style` and `explode`. Comparing
`swagger.yaml` with `../petstore/v2/openapi.yaml` shows the same findings
as the OpenAPI 3 versions do.
