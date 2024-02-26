# 0003. A unified IR with computed polarity

* **Status:** Accepted
* **Date:** 2024-02-26

## Context

Faultline compares contracts written in OpenAPI, JSON Schema, Protobuf,
GraphQL and Avro. Whether a change is breaking depends on *direction*: adding
a required field breaks request senders but not response readers; narrowing a
string's `maxLength` breaks callers when the string is an input and is safe
when it is an output. Existing tools encode direction implicitly inside
per-format rules, and each tool gets different corner cases wrong (OpenAPI
callbacks, `readOnly`, GraphQL input types).

## Alternatives

* **Per-format diffing.** One comparison engine per format, each with its own
  notion of direction. Simple at first; duplicates the hard parts (recursion,
  unions, constraint subsumption) five times and makes cross-format rules
  impossible.
* **A unified IR with direction declared by front ends.** Front ends tag every
  type with request/response. Types shared between both sides must then be
  duplicated or tagged "both" by each front end, repeating the reachability
  analysis in every one of them.
* **A unified IR with polarity computed from surfaces.** Front ends describe
  *where* values enter and leave the API (operations, schema roots); the IR
  derives the direction of every type by propagation.

## Decision

All formats lower into one IR (`faultline-ir`). Front ends declare surfaces
and the type graph; polarity is computed by `Polarities::compute`, which
propagates from every root, flips direction across callbacks and `not`, and
drops directions at `readOnly`/`writeOnly` nodes.

Positions are identified by `IrPath`, a textual identity that never contains
spans or arena indices.

## Reasoning

Direction is a property of *use*, not of the type definition, so it belongs
in one analysis over the whole graph. Computing it once makes the subtle cases
correct everywhere: a callback's request body is an output of the API; the
operand of `not` must *shrink* when the outer schema grows; a `readOnly`
subtree is not part of the request contract at all.

One graph traversal (`graph::for_each_edge`) defines the structure every
algorithm walks, which keeps polarity, validation, dumping and comparison
consistent with one another as type kinds are added.

The IR also records which recursive cycles are well-founded: every cycle must
pass through an object field, array item or similar constructor. This is
validated up front because the comparison engine's treatment of recursion is
only sound for such graphs.

## Trade-offs

* The IR must be expressive enough for every format, so some kinds (tagged
  unions, open enums, maps, wire tags) exist only for some front ends.
* Polarity is per type node. A node shared by request and response is *both*
  even if, along one particular path, only one direction matters; comparison
  refines this by carrying the direction of the path it is on.
* `IrPath` syntax is public and must stay stable once released.

## Consequences

* Rules can be written once against the IR and apply to every format.
* Front ends are responsible only for faithful lowering; they never reason
  about compatibility.
* New surface kinds (RPC methods, GraphQL root fields, message channels) are
  added to the IR alongside their front ends, together with their root
  polarity.
