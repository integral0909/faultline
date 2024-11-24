# Thrift

`faultline-thrift` reads Apache Thrift IDL files, with the files they
include, and lowers their types and services into the IR.

## Reading

The lexer and parser cover the whole IDL:

* headers: `include`, `cpp_include`, `namespace`;
* definitions: constants, typedefs, enums, structs, unions, exceptions,
  services with their functions;
* field ids, requiredness, default values and annotations;
* `/** */` doc comments.

Options that only affect generated code (`cpp_type`, `xsd_all` and
similar) are read and ignored. The deprecated `senum`, `slist` and `async`
are read with a warning (`FLE8009`). After a syntax error (`FLE8001`),
parsing resumes at the next definition.

`include "path"` is resolved against the including file's directory, and
then against the entry file's directory, which is where `thrift -I`
usually points. Each file is read once, so files that include each other do
not loop. An included file's definitions are named with the file's name
(`shared.Tag` for `shared.thrift`).

## Checks

The linker reports what the Thrift compiler would refuse:

| Code | Problem |
|---|---|
| `FLE8002` | an include that cannot be found |
| `FLE8003` | a type or service name that resolves to nothing |
| `FLE8004` | a definition, enum value or function defined twice |
| `FLE8005` | a field id outside 1 to 32767 or used twice, a field name used twice, an enum value outside `i32` |
| `FLE8006` | a name of the wrong kind: a service as a type, a struct thrown or extended, a `oneway` function with a result |
| `FLE8007` | a typedef or service that refers to itself |

It also warns about fields without an explicit id (`FLE8008`), whose
compiler-assigned ids change when fields are added or reordered, and about
enum values that share a number (`FLE8005`).

## Types

| Thrift | IR |
|---|---|
| `bool` | boolean |
| `byte`, `i8`, `i16`, `i32`, `i64` | integer with the type's range |
| `double` | number |
| `string`; `uuid` | string; string with format `uuid` |
| `binary` | bytes |
| `list<T>`, `set<T>` | array; a set's items are unique |
| `map<K, V>` | map |
| `enum` | the value names, each with its number |
| `struct`, `exception` | an object whose fields carry their ids as tags |
| `union` | an object whose fields form one exclusive group, of which one is set |
| `typedef` | the type it names |

Fields are identified by id, as on the wire, so renaming a field only
concerns the JSON protocol and generated code, while giving it another id
is removing it and adding another. A `required` field is required. Optional
fields and fields with the default requiredness are not, because readers
accept them missing.

Integers and collections carry their Thrift wire type as their *encoding*.
In Thrift, `i32` and `i64` are different wire types, and so are `list` and
`set`: a reader that expects one skips a field written as the other. So
changing between them is breaking (FL1031), even when every value would
still fit.

## Services

Each function of each service in the entry file is an operation
(`op[Pets/get]`), including the functions the service inherits through
`extends`. A client calls inherited functions the same way, so moving a
function to a parent service is not a change.

| Thrift | IR |
|---|---|
| arguments | the request: an open object of the arguments by id |
| return value and `throws` | the reply: a closed object with `success` (field 0) or one exception by id |
| `oneway` | a one-way operation, without a reply |

The reply is closed because a client fails a call whose result it cannot
read. Adding an exception is therefore risky: a client that receives it
fails the call. Removing an exception is harmless, and giving one another
id breaks clients that catch it. A function that becomes or stops being
`oneway` is breaking for its callers (FL3002).

Without services, every struct of the entry file is a schema root, used
in the role given with `--role`.

## Not modelled

* Constants: a changed constant changes generated code, not messages.
* Namespaces, which only affect generated code.
* Protocol differences: the model is the binary and compact protocols,
  where fields are identified by id. With the JSON protocols, field names
  matter as well; renames are still reported, as risky.
