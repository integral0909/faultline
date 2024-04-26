# The intermediate representation

`faultline-ir` defines the format-independent model every front end lowers
into and every later stage consumes. This page describes its parts and the
invariants they carry. The rationale is in
[ADR 0003](../design-decisions/0003-unified-ir-with-computed-polarity.md).

## Shape

```text
Api
├── operations: Vec<Operation>      HTTP operations and RPC methods
│   ├── parameters, request body    values flowing in
│   ├── responses                   values flowing out
│   └── callbacks                   nested operations, direction reversed
├── schemas: Vec<SchemaRoot>        standalone schemas with a declared role
├── types: TypeArena                the type graph
└── named: IndexMap<QualName, TypeId>
```

Types live in an arena and refer to each other by `TypeId`. The graph may be
cyclic; recursion is expressed with `Ref` nodes pointing at named types.
Front ends build recursive types by reserving an id before lowering the body
and defining it afterwards.

## Type kinds

Each `TypeKind` denotes a set of JSON-like values:

| Kind | Meaning |
|---|---|
| `Any`, `Never` | all values; no values |
| `Null`, `Boolean`, `Bytes` | the obvious sets |
| `Integer`, `Number` | numbers with inclusive/exclusive bounds, `multipleOf`, declared format and, for binary formats, wire encoding |
| `String` | length bounds (code points), ECMA-262 pattern, format |
| `Enum` | an explicit list of literals, with names and numbers where the format has them; *open* enums (Protobuf 3) document rather than restrict |
| `Object` | declared fields (with wire tags and aliases where the format has them), pattern properties, additional properties, property-name constraint, size bounds, dependent requirements, exclusive groups (Protobuf `oneof`: at most one member present) |
| `Array` | prefix items, item type, size bounds, uniqueness, `contains` |
| `Map` | typed keys and values (Protobuf, Avro) |
| `Union` | `Any` (anyOf), `Exclusive` (oneOf) or `Tagged` (discriminated by the data) |
| `AllOf`, `Not` | intersection and complement |
| `Ref` | transparent indirection |

Operations are keyed by method and path template for HTTP, and by
fully-qualified service and method for RPC (`pets.v1.PetService/GetPet`, the
gRPC method path). RPC operations record whether each direction is a stream.

Literals (`enum` members, `const`, defaults) are stored as `Literal`, whose
equality is JSON value equality: object keys are unordered and numbers are
exact decimals, so `1` and `1.0` are the same enum member.

## Traversal

`graph::for_each_edge` is the single definition of a node's children. Each
child is reached through an `Edge` that says how: `Field(name)`, `Items`,
`Variant(i)`, `Not`, `Ref`, and so on. Two properties of edges matter to the
algorithms:

* **Guarded** edges pass through a value constructor (fields, items, map
  values...). Unguarded edges (`Variant`, `AllOf`, `Not`, `Ref`) do not.
* **Inverting** edges (`Not`) reverse the direction of set inclusion.

## Polarity

`Polarities::compute` assigns every type node a subset of `{in, out}`:

1. Roots are seeded: parameters and request bodies are *in*, responses *out*;
   inside a callback these swap; schema roots use their declared role.
2. Polarity propagates along every edge, flipping across `Not`.
3. On arrival at a node marked `readOnly`, *in* is removed; at `writeOnly`,
   *out* is removed. The subtree below is only reached in the remaining
   directions.

Each node is processed at most once per direction, so the analysis is linear
even on cyclic graphs.

## Identities

`IrPath` names positions textually, for example

```text
op[GET /pets/{}]/response[200]/body[application/json].items[].name
```

Operation identities normalize path parameter names (`/pets/{id}` and
`/pets/{petId}` are the same endpoint), and header parameter names are
compared case-insensitively. The full grammar is documented on the
`faultline_ir::path` module; `Display` and `FromStr` are exact inverses,
which is property-tested.

## Invariants

`validate` checks, and later stages assume:

* every reserved type is defined and every edge target exists;
* no cycle consists only of unguarded edges (`A = allOf[A]` describes no
  values and would make recursive comparison unsound);
* object field names, enum members (by value), operation identities and root
  paths are unique;
* exclusive groups name declared fields, and no field is in two groups;
* `named` agrees with the names stored on the nodes.

## Dump format

`dump` renders an API deterministically: roots first, then every reachable
type in depth-first order, with named types labelled by name and others
`t1`, `t2`, ... in order of first visit. Output does not depend on arena
allocation order, which makes it suitable for snapshot tests and for
`faultline ir dump`.
