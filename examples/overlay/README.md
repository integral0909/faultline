# Overlay example

A pet store whose description also lists internal operations, marked
`x-internal`. Clients only see the public API: `public.yaml` is an overlay
(OpenAPI Overlay 1.0) that removes the internal operations, and the paths
left without operations.

```sh
faultline diff v1/openapi.yaml v2/openapi.yaml                        # 2 breaking
faultline diff v1/openapi.yaml v2/openapi.yaml --overlay public.yaml  # passes
```

Without the overlay, removing `DELETE /internal/pets/{id}` and requiring
`shard` on `POST /internal/reindex` are breaking changes. With it, only the
new `cursor` parameter of `GET /pets` is compared, and it is compatible.

In `faultline.toml`, overlays are listed per API and apply to both
versions:

```toml
[[api]]
name = "public"
entry = "v2/openapi.yaml"
overlays = ["public.yaml"]
```
