# OpenAPI lowering

`faultline-openapi` lowers Swagger 2.0 and OpenAPI 3.0, 3.1 and 3.2 descriptions into the IR
([ir.md](ir.md)). Schemas are delegated to the JSON Schema front end
([json-schema.md](json-schema.md)); this crate handles everything around them.

## Versions and dialects

| `openapi` | Schema dialect |
|---|---|
| `3.0.x` | the OpenAPI 3.0 Schema Object: draft-04-style bounds, no `$id`, `$ref` siblings ignored, `nullable` |
| `3.1.x`, `3.2.x` | `jsonSchemaDialect` if it names a known dialect, else JSON Schema 2020-12 |
| `swagger: "2.0"` | the Swagger 2.0 Schema Object: as OpenAPI 3.0, with `x-nullable` for `nullable` and `type: file` as a string |

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

## OpenAPI 3.2

* The `query` field of a path item is a `QUERY` operation
  (`QUERY /pets`).
* `additionalOperations` names methods without a field of their own. Every
  method Faultline distinguishes has one, so an entry for one of them is an
  error, and entries for other methods (`COPY`, `PURGE`, ...) are reported
  as not compared (`FLE2012`).
* A `querystring` parameter describes the whole query string with
  `content`. Each property of its object schema is a query parameter,
  required as the schema says, so moving parameters into a `querystring`
  is not a change.
* A media type's `itemSchema` describes each item of a sequential media
  type (JSON Lines, server-sent events). The content is compared as an array
  of those items.

## Swagger 2.0

Swagger 2.0 is lowered into the same IR, as the official converters
translate it to OpenAPI 3, so that a migration compares cleanly with the
description it replaces:

| Swagger 2.0 | IR (as OpenAPI 3) |
|---|---|
| `in: body` parameter | the request body, one content entry per media type in `consumes` (default `application/json`); its `required` |
| `in: formData` parameters | an object request body with one member per parameter, as `multipart/form-data` if consumed or if a parameter is a `file`, else `application/x-www-form-urlencoded` |
| `in: path`, `query`, `header` | parameters whose inline schema keywords (`type`, `items`, `enum`, bounds) are lowered in the Swagger dialect |
| `collectionFormat` of arrays | `csv`: `form`/`simple` unexploded; `multi`: `form` exploded; `ssv`, `pipes`, `tsv`: `spaceDelimited`, `pipeDelimited`, `tabDelimited`. Other parameters get OpenAPI 3's defaults. |
| response `schema` | one content entry per media type in `produces` |
| response `headers` | headers with inline schemas, not required |
| `definitions` | named types |
| `consumes`, `produces`, `security` | document-wide, overridden per operation |

`host`, `basePath` and `schemes` are not compared, just as OpenAPI 3
`servers` are not. A migration that keeps the wire format produces no
findings; one that drops Swagger's `csv` array format for OpenAPI 3's
exploded default is reported as a serialization change, because it is one.

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
| FLE2010 | a Swagger version other than 2.0 |
| FLE2011 | path template does not start with `/` |
