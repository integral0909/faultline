# GraphQL front end

`faultline-graphql` reads a GraphQL schema document (SDL, October 2021
specification) and lowers it into the IR.

## Parsing

The lexer follows the specification. Commas, comments and byte-order marks
are ignored. Block strings lose their common indentation and blank leading
and trailing lines. `\u{…}` escapes and surrogate pairs are decoded. Numbers
with leading zeros, or directly followed by a name, are errors.

The parser handles every type-system definition and extension:
* `schema`;
* `scalar`, `type`, `interface`, `union`, `enum` and `input`, with
  descriptions, directives and default values;
* `directive` definitions, including `repeatable`.

A syntax error abandons the definition and parsing resumes at the next
token that can start one. Queries and fragments in a schema document are
reported (`FLE4007`) and skipped. Nesting of list types and constant values
is limited to 128 levels.

## Schema building

`schema.rs` merges each `extend` into the type it extends; an extension of
the wrong kind or of an unknown type is an error. It resolves the root
operation types, from `schema { … }` or else the default names `Query`,
`Mutation` and `Subscription`, and then checks:

* type references resolve, and every name is defined once, including
  fields, arguments and enum values;
* fields have output types, and arguments and input fields have input types;
* union members are object types;
* an object or interface provides every field, with its arguments, of each
  interface it implements;
* no name starts with `__`, and no type is empty;
* a query root exists.

## Lowering

Each field of a root type becomes an operation, `query/pets`,
`mutation/addPet` or `subscription/petAdded`: its arguments are the request
body, its type is the response, and subscriptions stream their responses.

Values are modelled as a client sees them:

| GraphQL | IR |
|---|---|
| object type | named object; every field is required (a selected field is always present), and the object is open (unselected fields are absent) |
| `__typename` | a required field on every object: a one-value enum of the type name, or for an interface, of its implementations |
| field arguments | the field's `arguments`: a closed object whose values flow opposite to the field |
| `T` | `T | null` |
| `T!` | `T` |
| `[T]` | array of `T` |
| input object, argument list | closed object; a non-null value without a default is required; defaults are recorded |
| enum | closed enum of value names |
| union | tagged union of its members |
| interface | the object of its declared fields |
| `Int` | 32-bit integer |
| `Float` | number |
| `String`, `ID` | string (`ID` as its format) |
| custom scalar | string with the scalar's name as its format |
| `@deprecated` | deprecation of the field, argument or enum value |

## What the comparison sees

Because output fields are required members of open objects, removing a field
breaks clients that select it, and adding one is compatible. Making an output
nullable adds a `null` alternative and breaks clients. `__typename` makes
objects of different types distinct, so a new union member is a new value in
responses, shown with a witness such as `{"__typename": "Shelter", …}`, and a
new implementation of an interface is a new `__typename` value. Arguments
and input fields are judged as requests: a new required argument, a removed
argument or input field, or an argument made non-null breaks clients.

## Limitations

* One schema file per API. Schemas split across files must be combined
  first.
* Directive definitions and uses other than `@deprecated` do not affect the
  comparison.
* Custom scalars are assumed to be strings; a change from one custom scalar
  to another is reported as a format change.
* Renaming a type is visible only through `__typename`, as an enum value
  removed and one added.
