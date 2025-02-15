# Avro front end

`faultline-avro` reads Apache Avro schemas (`.avsc`), protocols (`.avpr`)
and IDL files (`.avdl`) and lowers them into the IR.

## Reading and checking

The schema is parsed with the shared spanned JSON reader, so every
diagnostic points at the attribute that caused it. The checks follow Avro's
parsers:
* named types (records, errors, enums, fixed) must be defined before they
  are used, and a name is defined once;
* names and symbols match `[A-Za-z_][A-Za-z0-9_]*`, and primitive names
  cannot be redefined;
* fields are unique within a record;
* a union may not contain another union or two branches of the same kind;
* a default matches the field's type. For a union, it must match the first
  branch: the classic mistake of `["string", "null"]` with a `null` default
  is reported;
* an enum default is one of its symbols;
* a logical type that does not fit its underlying type is a warning, since
  readers ignore it.

Namespaces follow the specification: a dotted name is full, and otherwise
the `namespace` attribute or the enclosing namespace applies.

## Two views of one schema

Avro compatibility is *schema resolution*: a reader schema reads data that
was written with a writer schema. Writers and readers see the same schema
differently:

| | written | read |
|---|---|---|
| record | exactly the declared fields, every one | undeclared fields ignored; missing fields with defaults filled in; fields matched by name or alias |
| enum | exactly the listed symbols | with a `default`, any symbol |

The IR describes the reader's view, and marks records and enums
`writer_exact`. Every inclusion check reads its left side as written and its
right side as read (`Validator::writer`, `Sampler::writer`). So "old values
the new version rejects" means data written with the old schema that the new
schema cannot read, which is Avro's definition of BACKWARD compatibility.
Other formats never set the flag, so for them the two views coincide.

The comparison role selects the compatibility mode:

| Role | Avro mode | Checked |
|---|---|---|
| `input` | BACKWARD | the new schema reads old data |
| `output` | FORWARD | the old schema reads new data |
| `both` | FULL | both |

## Lowering

| Avro | IR |
|---|---|
| record | named, open, `writer_exact` object; fields without defaults are required; aliases and docs kept |
| enum | named enum of symbols; open if it has a default; `writer_exact` |
| fixed | named string of exactly `size` characters, format `fixed` |
| array, map | array; map with string keys |
| union | tagged union of its branches |
| `int`, `long` | 32- and 64-bit integers |
| `float`, `double` | number, `float` marked by its format |
| `bytes`, `string` | string (`bytes` by format), since Avro resolves them into each other |
| logical types | the underlying type with the logical type as its format (`decimal(p,s)` with its precision and scale) |

Integer ranges make the promotions Avro allows widenings: `int` to `long`,
`float` or `double`; `long` to `float` or `double`; `float` to `double`.
Readers matching renamed fields through aliases are compared as the same
field, with the rename reported (FL1025).

## Protocols

A protocol defines named types in order, as a schema does, and messages.
Each message is an operation (`op[Orders/get]`):

* the request is a record of the message's parameters, with their
  defaults;
* the response is the response type or, when the message declares errors,
  a union of the response type and the errors. A new error is therefore a
  union branch that an old client's reader cannot resolve, which is
  breaking;
* a `one-way` message has no response and is a one-way operation, and
  changing that is breaking (FL3002).

A protocol without messages is compared as its named types, each a schema
root.

## IDL

Avro IDL files are read into the protocol tree an `.avpr` document gives,
with spans in the IDL, so the checks, the lowering and the findings are
those of the JSON forms. The reader covers:

* protocols with `@namespace`, and Avro 1.12's schema files (`namespace`,
  `schema` and declarations), read as protocols without messages;
* `record`, `error`, `enum` with a default, and `fixed`;
* messages with `throws` and `oneway`, and `void` responses;
* fields, several per declaration, with defaults and annotations
  (`@aliases`, `@order`);
* `array<T>`, `map<T>`, `union { ... }` and `T?`. `T?` is a union with
  `null`, placed first unless the field's default is something else;
* the logical types `date`, `time_ms`, `timestamp_ms`,
  `local_timestamp_ms`, `uuid` and `decimal(p, s)`;
* doc comments (`/** */`), and names in backquotes;
* `import idl`, `import protocol` and `import schema`, resolved against the
  importing file. Imported types keep their own namespace. A file that
  imports itself, directly or through others, is reported.

## Limitations

* Readers reject a writer's `long` or an unknown enum symbol outright. These
  are reported as risky rather than breaking, by the same rule that applies
  to values a response may now contain in every format. Raise FL1005 and
  FL1007 with severity overrides where a stricter policy is wanted.
* Removing a default from a field is reported as the field becoming
  required, although Avro writers always write every field.
* Transitive modes (checking against every earlier version) need each
  earlier version compared in turn.
* `.avsc` schemas referencing named types from other files or a registry
  are not resolved; one self-contained schema per API, or an IDL file that
  imports the others.
