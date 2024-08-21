# Protobuf front end

`faultline-protobuf` reads `.proto` files (proto2, proto3 and editions) and
lowers them into the IR. It has four stages, each in its own module.

## Lexing and parsing

The lexer (`lexer.rs`) never fails: malformed input becomes an error token
that the parser reports where it appears. String literals are decoded to
bytes, since Protobuf strings may hold arbitrary octets. Comments directly
before a declaration, with no blank line between, become its documentation;
a comment after a token on the same line describes that token and is dropped.

The parser (`parser.rs`) is recursive descent over the tokens. Protobuf
keywords are contextual: `message message = 1;` declares a field named
`message` of type `message`. Statements are recognized by lookahead instead:
`message Name {` starts a nested message, anything else in a message body is
a field. As in `protoc`, `option` followed by a name is always an option.

A syntax error abandons the statement: the parser skips to the next `;` at
the same level, or to the `}` of the enclosing block, and continues. One typo
produces one diagnostic. Nesting deeper than 128 levels, of declarations or
of braces in option values, is reported, not followed, so hostile input
cannot exhaust the stack.

## Loading

`load.rs` reads the entry file, then its imports depth first, iteratively.
Import paths are relative to an import root. The roots tried are the entry's
directory and each ancestor up to the workspace root, nearest first, which
covers the usual `protoc -I proto` layout without configuration.
`google/protobuf/*.proto` (the well-known types and `descriptor.proto`) and
`google/api/{annotations,http,field_behavior}.proto` are built in. Import cycles are reported, not followed, and more than 10,000
files is an error.

Files are read through the same `Loader` as other formats, so the workspace
sandbox and git revisions apply unchanged.

## Linking

`linker.rs` assigns every message, enum, service and package a
fully-qualified name and resolves type references the way `protoc` does:

* A name starting with `.` is absolute.
* Otherwise the first component is looked up from the innermost scope
  outwards. The first scope that has it decides, and the rest of the name
  must exist inside it. So in package `a.b`, with a message `c`, the name
  `c.D` means `a.b.c.D`, even when a package `c` contains `D`.
* A reference may only name types from its own file, from files it imports,
  and from files those re-export with `import public`.

It also checks what `protoc` rejects:
* numbers: range, the implementation range 19000–19999, duplicates, and
  reserved numbers and names;
* map key types;
* kinds: an RPC input must be a message, and a field type must be a message
  or an enum;
* rules of the syntax level: labels in proto2; no `required`, groups or
  defaults in proto3; a zero first value for open enums;
* enum value names, which are scoped like C++ enumerators.

## Lowering

`lower.rs` describes the binary wire format:

| Protobuf | IR |
|---|---|
| message | named object; unknown fields are skipped, so the object is open |
| field | field with its number as the tag; the JSON name as an alias |
| `required` (proto2, or `LEGACY_REQUIRED`) | required field |
| `repeated T` | array of `T` |
| `map<K, V>` | map |
| `oneof` | exclusive group |
| integer types | integer with the type's range, its name as the format, and its wire encoding (`varint`, `zigzag`, `fixed32`, `fixed64`) |
| `float`, `double` | number with encoding `float` or `double` |
| enum | enum of value names with their numbers; open unless proto2 or `features.enum_type = CLOSED` |
| `reserved` numbers and names | retired tags and names of the object or enum |
| `[deprecated = true]`, `[default = …]`, leading comments | field or type metadata |
| service method | RPC operation `package.Service/Method`; request and response bodies of media type `application/grpc`; streaming per direction |

A file with services is an API of operations. A file without services
describes data: its top-level messages become schema roots, used in the
role the configuration gives (`both` by default). Every message and enum the
entry file declares is compared, whether or not it is used.

## What the comparison sees

Because fields carry tags, the diff matches message fields by number, not
by name. Renaming a field keeps its data on the wire (FL1025, risky for JSON
and generated code). A field whose number changed is reported as that move
(FL1026). Enum values are matched by number and by name, which separates
renumbering (FL1029) from renaming (FL1030). Other checks:
* changes in wire encoding (FL1031), for example `int32` to `sint32`;
* changes in value range, with witnesses, for example `int32` to `int64`;
* moves into and out of `oneof`s (FL1028);
* RPCs that become or stop being streams (FL3001);
* reservations: a removed field or enum value whose number is not reserved
  (FL1032), a new one that reuses a reserved number or name (FL1033), and a
  dropped reservation (FL1034).

## The HTTP/JSON view

With `format = "protobuf-http"`, `lower_protobuf_http` lowers the entry
file's services as the HTTP/JSON API their `option (google.api.http)` rules
expose, the API an OpenAPI description of a transcoded service describes.
`http.rs` parses each rule's text-format value and builds one operation per
binding (the rule and each of its `additional_bindings`):

| Rule | Operation |
|---|---|
| `get: "/v1/{name=pets/*}"` | `GET /v1/{name}`; `name` a required path parameter |
| `body: "*"` | the request message, less top-level path fields, as the body |
| `body: "pet"` | the `pet` field as the body; other fields are query parameters |
| no `body` | every field not in the path is a query parameter; singular nested messages are flattened into dotted names (`filter.species`), repeated messages and maps are not expressible |
| `response_body: "pet"` | that field of the response as the `200` body; otherwise the whole response message |

Operation ids are `Service_Method`. RPCs without a rule and streaming RPCs
are not exposed; rules that name missing fields, custom methods and paths
bound twice are warnings (FLE3013). Bodies are optional, since an empty
body decodes as an empty message.

`json.rs` types the view by the proto3 JSON mapping: JSON field names,
64-bit integers and the 64-bit wrappers as strings, enums as their value
names, `bytes` as `format: byte` strings, maps as objects, `Timestamp` as a
`date-time` string, `Struct` and `Value` as any object and any value. No
field is required unless annotated `(google.api.field_behavior) =
REQUIRED`, because printers omit default values. 32-bit integers carry
`format: int32` (`uint32`: `int64`) rather than bounds, as OpenAPI
descriptions write them. The view describes what the service prints;
parsers are more lenient (they also accept numbers for 64-bit integers and
enums).

Comparing an OpenAPI description (old) with the view (new) reports the
description's promises the service does not keep. The comparison is the
ordinary one, so a documented operation without a binding is removed, a
field whose type changed is a type change, and so on.

## Limitations

* Custom options and extensions are parsed and checked but, apart from
  `google.api.http` and `google.api.field_behavior` in the HTTP view, do
  not affect the comparison.
* `map<K, V>` and the equivalent `repeated` entry message are not treated as
  the same type.
* Of the editions features, only `field_presence = LEGACY_REQUIRED` and
  `enum_type` are interpreted.
