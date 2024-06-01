# 0009. GraphQL as selections, with field arguments in the IR

* **Status:** Accepted
* **Date:** 2024-06-01

## Context

GraphQL does not fit the request/response picture the IR was built for.
There is one endpoint, and clients choose what they receive: a response
contains exactly the fields a query selected. Fields deep inside a response
take arguments, which the client sends, so values flow both ways inside one
response type. Clients also tell the members of a union apart by type
name, not by shape.

Three questions follow. What is an operation? How are nested arguments
represented? When does adding or removing a field break a client?

## Alternatives

For nested arguments:

* **Flatten them into the operation**, one request parameter per argument
  path. That loses the link between an argument and the field it belongs to,
  and does not work for recursive types.
* **Wrap arguments in `not`**, which already inverts polarity. That is wrong:
  `not` means the complement of a set, and would corrupt every value-level
  judgement.
* **Give fields arguments**: an object attached to the field, reached
  through its own edge, across which polarity inverts.

For field presence: model output fields as optional, as JSON Schema
responses usually are, or as required members of an open object.

## Decision

* Each root field is an operation (`query/pets`), with its arguments as the
  request and its type as the response.
* `Field.arguments` is part of the IR. The `Arguments` edge is guarded and
  inverts polarity. Its path segment is written `.field()`.
* Output fields are required members of open objects. A selected field is
  always present, and fields that were not selected are absent. Input
  objects and argument lists are closed.
* Every object carries a required `__typename` field, an enum of the names
  its values can have.

## Consequences

* Removing an output field breaks clients (a value without it is new), and
  adding one is compatible. Both follow from value-set reasoning, without
  GraphQL-specific rules.
* New union members and interface implementations are reported with
  witnesses showing the new `__typename`. They are classified as breaking,
  because a client without a fragment for the type gets nothing for it.
  Teams that treat this as merely risky can override FL1012.
* The differ derives the direction of an added or removed field from whether
  undeclared members are allowed. This also sharpens OpenAPI results: a field
  added to an open response object is now provably compatible.
