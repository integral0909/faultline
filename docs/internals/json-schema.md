# JSON Schema lowering

`faultline-jsonschema` turns JSON Schema documents (draft-04 through 2020-12)
into the IR described in [ir.md](ir.md). OpenAPI reuses the same machinery for
its schema objects.

## Pipeline

1. **Load.** Documents come from a `DocumentStore` (parsed once, cached,
   sandboxed; see [ADR 0004](../design-decisions/0004-sandboxed-offline-reference-resolution.md)).
2. **Scan.** The registry walks each schema, visiting exactly the subschema
   positions of each keyword for the active dialect, and records for every
   schema its base URI, dialect and enclosing resource, plus every `$id`,
   `$anchor` and `$dynamicAnchor`. Position matters: a property named `$id`
   is not an identifier.
3. **Lower.** Each schema location becomes one IR node. Asking for the node of
   a subschema or reference target reserves an id and queues the location; a
   single loop builds queued schemas. Nothing recurses, so reference chains of
   any length use constant stack, and recursive schemas simply receive their
   own reserved id.
4. **Validate.** The IR invariants are checked; cycles that never pass through
   a property or array item are reported as `FLE1009`.

## Dialects

| Behaviour | draft-04 | draft-06 | draft-07 | 2019-09 | 2020-12 |
|---|---|---|---|---|---|
| identifier keyword | `id` | `$id` | `$id` | `$id` | `$id` |
| anchors | `id: "#name"` | `$id: "#name"` | `$id: "#name"` | `$anchor` | `$anchor`, `$dynamicAnchor` |
| siblings of `$ref` | ignored | ignored | ignored | applied | applied |
| boolean schemas | — | ✓ | ✓ | ✓ | ✓ |
| exclusive bounds | booleans | numbers | numbers | numbers | numbers |
| tuples | `items: [...]` + `additionalItems` | same | same | same | `prefixItems` + `items` |
| dependencies | `dependencies` | same | same | `dependentRequired`/`dependentSchemas` | same |
| dynamic references | — | — | — | `$recursiveRef` | `$dynamicRef` |

The dialect comes from `$schema` at a resource root, else from the referencing
schema, else from the configured default (2020-12). Unknown `$schema` values
produce a warning and keep the inherited dialect.

## How keywords lower

A schema object is the conjunction of its parts, each lowered separately:

* **Typed part.** `type` plus the keywords specific to one type. A keyword
  only constrains values of its own type, so `{"maxLength": 3}` becomes a
  union over all six JSON types in which only the string variant is
  constrained. `integer` is folded into `number` when both are allowed.
* **`enum`/`const`** become enums. If the typed part only restates the type of
  every member (`type: string, enum: [a, b]`) it is dropped.
* **`allOf`** members are conjuncts; **`anyOf`/`oneOf`** become inclusive and
  exclusive unions; **`not`** becomes a negation.
* **`if`/`then`/`else`** becomes `(if ∧ then) ∨ (¬if ∧ else)`; a missing branch
  is `true`, and `if` alone has no effect.
* **Dependent schemas** `{p: S}` become `(p absent) ∨ S`.
* **`required`** names without a declared property still become required
  fields, typed by `additionalProperties`.

One part becomes the node itself; several become an `AllOf`.

## Reference resolution

References resolve against the base URI of the innermost resource. Fragments
starting with `/` are JSON Pointers relative to that resource; other fragments
are anchor names. A reference to a document not yet seen loads it through the
store and scans its root. Locations that are not subschema positions of
anything scanned (for example a schema inside `x-` extension data) are scanned
on demand, inheriting their nearest scanned ancestor's base.

### Dynamic scope

`$dynamicRef` and `$recursiveRef` resolve to the *outermost* resource in the
dynamic scope that declares the matching anchor, so one location can denote
different types depending on how it was reached. The standard example is a
"strict tree" that extends a "tree" so that its children are strict trees as
well.

Lowering keys its memoization by location **and** by the bindings the current
scope implies for every anchor declared in more than one resource. Documents
without rebindable anchors get empty bindings, so this costs nothing for them.
Specialized copies are named after their binding (`tree@strict-tree`).

## Naming

Named IR types are definitions (`$defs`, `definitions`), document roots (named
after the file) and identified resources. The entry document's definitions
are named first. A name already used by another location is qualified with
its file (`common.Item`), then with its full location.

## Approximations

`unevaluatedProperties` and `unevaluatedItems` depend on which members other
subschemas evaluated for a particular instance. Faultline treats every member
named by an in-place subschema (reached through `allOf`, `$ref`, `anyOf`,
`oneOf`, conditionals or dependent schemas) as evaluated. This is exact when
only `allOf` and `$ref` are involved. With branches, the IR accepts more
values than the schema, and the `FLE1010` warning says so.

## Diagnostics

| Code | Meaning |
|---|---|
| FLE1001 | invalid keyword value (the keyword is ignored) |
| FLE1002 | unknown type name |
| FLE1003 | reference does not resolve |
| FLE1004 | reference points outside the workspace |
| FLE1005 | remote reference while network access is disabled |
| FLE1006 | referenced document cannot be loaded or parsed |
| FLE1007 | unknown `$schema` dialect |
| FLE1008 | keywords next to `$ref` ignored (draft-07 and earlier) |
| FLE1009 | schema defined in terms of itself without a property or item in between |
| FLE1010 | keyword approximated |
| FLE1011 | reference points at something that is not a schema |
| FLE1012 | duplicate `$id` or anchor |
| FLE1013 | invalid URI in `$id` or `$ref` |
