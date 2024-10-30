# RAML

`faultline-raml` reads RAML 1.0 APIs and lowers their resources and methods
into operations, and their data types into the IR's type graph.

## Loading

A RAML document is YAML whose first line names the version and kind of
document: `#%RAML 1.0` for an API, `#%RAML 1.0 Library` for a library, and
so on. Other versions, including RAML 0.8, are reported as unsupported
(`FLE7602`). The YAML loader keeps RAML's local tags, so `!include path`
embeds another file where it occurs: a RAML or YAML file as its tree, a JSON
file as its parsed value, and any other file as its text. Includes are
resolved against the including file and confined to the workspace. A file
that includes itself, directly or indirectly, is reported (`FLE7603`).
Included nodes keep the spans of the files they came from, so findings point
into the included file.

## Libraries

`uses: {common: lib/common.raml}` loads a library. Its types, traits and
resource types are then available as `common.Pet`, `common.paged` and
`common.collection`. A library can use other libraries, whose declarations
are qualified in turn (`outer.inner.Pet`), up to 16 levels deep. Inside a
library, its own templates name each other without the qualifier. They are
qualified when the library is loaded, so a resource type borrowed from
`common` still finds `common`'s `paged` and not a trait with the same name
declared by the API. Types in a library are named with the qualifier in
findings.

## Resource types and traits

Before a resource is lowered, its resource type (`type`) and traits (`is`)
are merged into it:

* Parameters are replaced first: those given where the template is named
  (`type: {collection: {item: Pet}}`), plus `<<resourcePath>>`,
  `<<resourcePathName>>` and `<<methodName>>`. The transforms `!singularize`,
  `!pluralize`, `!uppercase`, `!lowercase` and the camel, snake and kebab
  case transforms can be applied to a parameter
  (`<<resourcePathName | !singularize>>`).
* Merging keeps the values a resource or method already has, merges maps
  member by member, and fills an empty method (`delete:`) entirely from the
  template.
* A method a resource type declares as optional (`get?:`) is merged only
  into a method the resource already has.

Precedence, from highest to lowest:

1. the method itself;
2. the method's traits, in the order listed;
3. the resource's traits;
4. the resource type, then the type it names in turn (at most 16 levels).

The result is compared as if it had been written out in place. A change to a
trait therefore shows up at every operation that uses it.

## Operations

Nested resources join their paths: `/pets` containing `/{petId}` is
`/pets/{petId}`. Each method is an operation (`op[GET /pets/{}]`) and points
at its method key.

| RAML | IR |
|---|---|
| `uriParameters` (on the resource or an ancestor) | path parameters; variables in the path that are not declared are strings |
| `queryParameters`, the properties of a `queryString` type | query parameters |
| `headers` | header parameters |
| `body` keyed by media type | a request body with one content entry per media type |
| `body` given directly as a type | one entry for each of the API's `mediaType`s (`application/json` by default) |
| `responses` keyed by status code | responses with their headers and bodies |
| `securedBy` | security requirements; `null` in the list means no credentials are needed |

Parameters are required unless their name ends in `?` or they are declared
with `required: false`. URI parameters are always required.

## Types

| RAML | IR |
|---|---|
| `any` | any value |
| `string` with `pattern`, `minLength` and `maxLength` | string |
| `number` and `integer` with `minimum`, `maximum`, `multipleOf` and `format` | number or integer; `format` (`int8` to `int64`, `long`, ...) limits the range |
| `boolean`, `nil` | boolean, null |
| `date-only`, `time-only`, `datetime-only`, `datetime` | string with a format |
| `file` | string with format `binary` |
| `array`, `T[]` with `items`, `minItems`, `maxItems` and `uniqueItems` | array |
| `object` with `properties`, `additionalProperties`, `minProperties` and `maxProperties` | object |
| `A \| B`, `T?` | union; union with null |
| `enum` | the listed values |

A property is required unless its name ends in `?` or it is declared with
`required: false`. A property named `/regex/` is a pattern property. Objects
are open unless they declare `additionalProperties: false`. A type that names
other types in `type` inherits from them: an object gets their properties,
with its own properties taking precedence, and a scalar gets their facets.

An object type with a `discriminator`, declared or inherited, has the
property it names fixed to its `discriminatorValue` (the type's name unless
given) and required. A union of such types, `Cat | Dog`, is discriminated by
that property: its variants are matched by value. Changing a type's value
breaks requests that send the old one, and in responses a new value is risky.

A type can also be a JSON Schema, included from a `.json` file
(`Pet: !include schemas/pet.json`) or written inline as JSON text. JSON
Schema's rules apply to it, not RAML's: a property is optional unless
`required` lists it, and objects are open. The RAML lowering leaves a
placeholder for each schema. Afterwards the JSON Schema front end
([json-schema.md](json-schema.md)) lowers the schemas, draft-04 unless they
declare `$schema`, into a type graph that is grafted onto the API's. Each
placeholder then refers to its schema. `$ref`s resolve against the schema's
own file, or against the API's file for inline schemas. A file used in
several places is one type. Because RAML allows no facets on a JSON Schema
type, a type based on one is that schema.

Named types keep their names. A type used as a request body and as a
response is one type in the IR, and its polarity comes from where it is used
(see [ir.md](ir.md)).

## Diagnostics

| Code | Meaning |
|---|---|
| `FLE7601` | not a RAML document |
| `FLE7602` | a RAML version other than 1.0 |
| `FLE7603` | an include cannot be loaded, is outside the workspace, or includes itself |
| `FLE7604` | a malformed declaration, including a JSON Schema combined with other types; also a warning for inline XML schemas |
| `FLE7605` | an unknown type, trait, resource type or library, or a resource type inheriting too deeply |
| `FLE7606` | a type expression that cannot be parsed |

## Not modelled

* XML schemas used as types are compared as accepting any value, with a
  warning (`FLE7604`).
* `baseUri`, `protocols` and the security schemes' own settings: these are
  deployment details, not message shapes.
* Annotations, examples, overlays and extensions.
