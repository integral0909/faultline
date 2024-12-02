# JSON Type Definition

`faultline-jtd` reads JSON Type Definition schemas (RFC 8927) and lowers
them into the IR: the schema is one schema root, and each definition is a
named type, compared whether or not the root uses it.

## Checks

A JSON Type Definition is strict, and so is the front end. Each schema
uses the keywords of exactly one form, plus `nullable` and `metadata`.
Only the root may have `definitions`. Unknown keywords are errors, as they
are for RFC 8927 implementations.

| Code | Problem |
|---|---|
| `FLE8201` | an unknown keyword, a keyword of the wrong type, keywords of several forms, `definitions` below the root, an unknown `type`, an empty `enum` |
| `FLE8202` | a `ref` to no definition |
| `FLE8203` | an enum value listed twice, a property both required and optional |
| `FLE8204` | a discriminator mapping that is not a non-nullable properties schema, or that declares the discriminator itself |
| `FLE8205` | definitions that are only `ref`s to each other |

## Forms

| Form | IR |
|---|---|
| empty (`{}`) | any value |
| `type: boolean`, `string` | boolean, string |
| `type: timestamp` | string with format `date-time` |
| `type: float32`, `float64` | number with that format |
| `type: int8` … `uint32` | integer with the type's range and name as its format |
| `enum` | the listed strings |
| `elements` | array |
| `properties`, `optionalProperties` | object, closed unless `additionalProperties: true` |
| `values` | object whose members all have one type |
| `discriminator` and `mapping` | tagged union: each variant is its mapping's object with the discriminator property fixed to its value |
| `ref` | the named definition |
| `nullable: true` | the form's type or `null` |

A description in `metadata.description` is kept as documentation.

## Consequences for findings

JTD objects are closed by default, and validators reject members they do
not know. So a property added to data that consumers validate is risky
even when it is optional, and a new mapping in a discriminator is breaking
for consumers, who cannot read the new variant. Integer types are also the
types of generated code: widening `uint8` to `uint16` accepts every old
value, but is reported as a format change.

Files ending in `.jtd.json` are recognized as JSON Type Definitions, and
the API is named without the compound extension. Other file names need
`--format jtd`, because a JSON Type Definition cannot be told from a JSON
Schema by its content.
