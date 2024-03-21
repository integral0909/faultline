# OpenAPI lowering

`faultline-openapi` lowers OpenAPI 3.0 and 3.1 descriptions into the IR
([ir.md](ir.md)). Schemas are delegated to the JSON Schema front end
([json-schema.md](json-schema.md)); this crate handles everything around them.

## Versions and dialects

| `openapi` | Schema dialect |
|---|---|
| `3.0.x` | the OpenAPI 3.0 Schema Object: draft-04-style bounds, no `$id`, `$ref` siblings ignored, `nullable` |
| `3.1.x` | `jsonSchemaDialect` if it names a known dialect, else JSON Schema 2020-12 |
| `swagger: "2.0"` | not supported yet (`FLE2010`) |

`nullable: true` adds `null` to the declared `type` and has no effect without
one, following the OpenAPI 3.0.3 clarification; an `enum` without `null` still
rejects it.

## What is lowered

* **Operations** for every HTTP method of every path item, keyed by method
  and normalized template (`GET /pets/{}`). Two templates that differ only in
  parameter names are the same endpoint (`FLE2004`).
* **Parameters**: path-level parameters are inherited and overridden by
  operation-level ones with the same location and name. Header parameters
  named `Accept`, `Content-Type` and `Authorization` are ignored, as the
  specification requires. Every template variable needs a required path
  parameter and vice versa (`FLE2005`).
* **Serialization** is stored with defaults applied — `form`/explode for query
  and cookie parameters, `simple` for path and header ones — so spelling a
  default out is not a change.
* **Request bodies and responses** by media type (lowercased, whitespace
  normalized) and status (`200`, `4XX`, `default`). `Content-Type` response
  headers are ignored.
* **Callbacks** as nested operations, and 3.1 **webhooks** at the top level;
  both reverse the direction values flow.
* **Security**: the operation's own `security`, else the document's. An empty
  list means no credentials are needed.
* **Schemas**: every schema position, plus every `components/schemas` entry as
  a named type whether or not anything uses it.
* **Discriminators**: explicit `mapping` entries (by reference or component
  name), and implicit mappings from the component names of `$ref` variants
  that no explicit entry already targets.

## References

Reference Objects between non-schema objects (parameters, responses, headers,
path items, callbacks, request bodies) are followed through the same
sandboxed document store as schema references. Chains are followed for up to
32 hops; cycles are reported (`FLE2008`).

## Diagnostics

| Code | Meaning |
|---|---|
| FLE2001 | not an OpenAPI 3 description |
| FLE2002 | a field has the wrong type |
| FLE2003 | unresolved reference |
| FLE2004 | duplicate operation |
| FLE2005 | path parameter problems |
| FLE2006 | invalid response status key |
| FLE2007 | invalid parameter location |
| FLE2008 | reference cycle |
| FLE2009 | parameter or header without schema or content |
| FLE2010 | Swagger 2.0 not supported |
| FLE2011 | path template does not start with `/` |
