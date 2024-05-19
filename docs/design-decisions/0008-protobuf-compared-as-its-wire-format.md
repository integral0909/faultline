# 0008. Protobuf compared as its wire format

* **Status:** Accepted
* **Date:** 2024-05-19

## Context

A Protobuf schema has three audiences with different compatibility needs:

* the **binary wire format**, which identifies fields by number and enum
  values by number;
* the **JSON mapping**, which identifies both by name;
* **generated code**, which uses names throughout.

Renaming a field is harmless to the first and breaks the other two.
Renumbering a field is the reverse. The IR's object model was built for JSON,
where members are identified by name.

## Alternatives

* **A Protobuf-specific comparison** beside the IR, as dedicated
  Protobuf linters do. This is precise, but it duplicates the whole
  comparison engine, and Protobuf findings would get no value-level evidence:
  no witnesses for narrowed integer ranges, no reasoning about `oneof`s.
* **Lower Protobuf as JSON.** Names as member names, with the wire format
  ignored. Most Protobuf traffic is binary, so the most damaging breaks
  (renumbered fields and enum values, changed encodings) would go unreported.
* **Lower the wire format into the IR and extend the IR** with what the wire
  needs: field tags, enum numbers, wire encodings, exclusive groups and RPC
  operations. The comparison matches fields by tag when both sides have
  them, and reports the name and JSON aspects as separate findings.

## Decision

The third. Messages are open objects whose fields carry tags, with JSON names
as aliases. `oneof`s are exclusive groups, which the validator, sampler and
checker understand, so moving fields into a group comes with a verified
witness. Integers carry their range and wire encoding. Services are RPC
operations with streaming recorded.

Renames of fields and enum values are risky rather than breaking: the binary
format, the common case for gRPC, is unaffected, while JSON users and
generated code are affected. Renumbering and encoding changes are breaking.

## Consequences

* The same engine, evidence and reporters serve Protobuf, and the new IR
  concepts (tags, aliases, exclusive groups, encodings) are available to Avro
  and GraphQL.
* Teams that use the JSON mapping exclusively should raise the rename rules
  (FL1025, FL1027, FL1030) to breaking with severity overrides.
* Differences that exist only in the JSON mapping, such as `int64` being a
  string in JSON, are not modelled.
