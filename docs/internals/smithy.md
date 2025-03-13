# Smithy

`faultline-smithy` reads Smithy IDL 2.0 models and lowers their services
into operations, or, for a model without services, its structures and
unions into schema roots.

## Reading

A Smithy model is usually spread over the files of a directory. The entry
file is read together with the other `.smithy` files beside it, from the
working tree or from a git revision. A model spread over several
directories is named by its `smithy-build.json`: the files its `sources`
and `imports` list are read, and so are the `.smithy` and JSON files
directly in the directories they list. Of those, the file declaring a
service is the entry. The lexer and parser cover the IDL:

* control statements, metadata, `namespace` and `use`;
* every shape type, with traits and their node values;
* mixins (`with [...]`), `for` resources and elided members (`$name`);
* default values, service and resource properties;
* operations with inline inputs and outputs (`input := { ... }`);
* `apply` statements and documentation comments (`///`).

After a syntax error (`FLE8401`), parsing resumes at the next statement.

A `.json` entry, or any JSON document with `smithy` and `shapes` members,
is a model in the JSON AST form that Smithy's build tools write. It is read
into the same syntax trees as the IDL, one per namespace. The namespace with
a service comes first, so the same model in either form is compared the
same way.
Models declaring a version other than 2 are read as version 2, with a
warning (`FLE8405`).

## Resolution

Relative shape ids resolve through the file's `use` statements, then the
file's namespace, then the prelude. Trait ids resolve the same way, and an
unknown trait is taken to be a prelude trait. Resolution then flattens the
model:

* Inline inputs and outputs become shapes (`GetCityInput`).
* A shape's mixins contribute their members, first and in order, and their
  traits, except `@mixin` itself. Redeclaring a mixin's member adds traits
  to it.
* An elided member takes its target from a mixin's member, or from the
  identifiers and properties of the resource named with `for`.
* `apply` adds traits before mixins are expanded, so shapes that use a
  mixin see traits applied to its members.

| Code | Problem |
|---|---|
| `FLE8402` | a shape id that resolves to nothing |
| `FLE8403` | a shape, or a member of a shape, defined twice |
| `FLE8404` | a shape of the wrong kind: an operation whose input is not a structure, an error without `@error`, a mixin without `@mixin` |
| `FLE8406` | a shape that is its own mixin, an elided member without a target |

## Shapes

| Smithy | IR |
|---|---|
| `blob` | bytes |
| `boolean` | boolean |
| `document` | any value |
| `string` | string, with `@pattern` and `@length` |
| `byte`, `short`, `integer`, `long` | integer with the type's range, narrowed by `@range` |
| `float`, `double`, `bigDecimal` | number, bounded by `@range` |
| `bigInteger` | integer |
| `timestamp` | string with format `date-time`, or a number with `@timestampFormat("epoch-seconds")` |
| `enum`, `intEnum` | the values (`NAME = value`, `@enumValue`, or the name) |
| `list` | array; `@uniqueItems` makes items unique, `@sparse` lets them be null |
| `map` | object whose members all have the value's type, names constrained as the key is |
| `structure` | open object; `@required` members are required |
| `union` | closed object with exactly one member set |

Structures are open because Smithy clients and servers ignore members they
do not know. Unions are closed, because a union whose member is unknown
has no value a reader can use: a new union member is risky for readers.
Documentation, `@deprecated` and defaults are kept.

## Operations

A service's operations are those it lists and those of its resources,
child resources included. An operation with `@http` is an HTTP operation,
bound by the HTTP binding traits:

| Trait | IR |
|---|---|
| `@httpLabel` | path parameter; greedy labels (`{key+}`) are plain ones |
| `@httpQuery("name")` | query parameter |
| `@httpHeader("Name")` | request or response header |
| `@httpPayload` | the body, with `@mediaType` or the default for its kind |
| members bound to nothing | a JSON body object |
| `@http(code: ...)` | the success status, 200 by default |

An operation without `@http` is a remote procedure call
(`op[Weather/GetCurrentTime]`) whose request is its input and whose
response its output. Errors, the operation's and the service's common
ones, are responses with the status of `@httpError`, or 400 for
`@error("client")` and 500 for `@error("server")`. Errors that share a
status are the alternatives of one response. An `@httpPayload` whose target
is `@streaming` makes that direction a stream.

## Not modelled

* Protocol-specific serialization, such as `@jsonName`, `@xmlName` or the
  AWS JSON protocols' `X-Amz-Target` routing. The model is compared as its
  shapes and HTTP bindings.
* `@httpQueryParams` and `@httpPrefixHeaders` maps.
* Files in subdirectories of the directories `smithy-build.json` lists;
  list those directories too.
