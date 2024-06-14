# Comparison

Comparing two versions of an API happens in two layers:

* `faultline-subsume` decides *inclusion* between types: is every value of
  one type a value of the other?
* `faultline-diff` walks both versions in parallel, records what changed at
  each position, and uses the inclusion checker to attach evidence about which
  values were lost and gained.

The design rationale is in
[ADR 0005](../design-decisions/0005-three-valued-inclusion-with-verified-witnesses.md).

## Validator

`Validator::validate(type, value)` returns `Valid`, `Invalid(reason)` or
`Unknown`. It is exact for every construct except:

* patterns that are not regular (backreferences, lookaround, word
  boundaries, Unicode property escapes) or whose automata exceed the size
  limits, and member names matched against `patternProperties`;
* `multipleOf` beyond exact `i128` rational range.

`pattern` is decided by `faultline-regex` (see below).

`format` is an annotation and never invalidates a value. Open enums (Protobuf
3) accept any value of the same JSON type as their members.

The validator is property-tested against an independent JSON Schema
implementation on random schemas and values.

## Regular expressions

`faultline-regex` compiles an ECMA-262 pattern with JSON Schema's search
semantics (a string must *contain* a match) into a DFA over code points:

1. The parser accepts the specification's syntax with the Annex B
   leniencies validators accept. It rejects backreferences, lookaround,
   word boundaries and property escapes as unsupported, and quantifier
   bounds above 1000.
2. A Thompson NFA reads character-set edges. `^` and `$` become assertion
   edges that subset construction honours by tracking whether input has
   been read yet and whether a `$` has passed, so anchors work anywhere
   in a pattern.
3. Subset construction runs over the coarsest partition of the code points
   the pattern distinguishes. It is capped at 5,000 DFA states and a
   budget on the total size of their NFA state sets.

`difference(a, b, min, max)` runs a breadth-first search over pairs of
states, with the length so far when bounds apply. It returns the shortest
string in `a` and not in `b`, or proves that there is none. The checker
compares strings with it: patterns are included or refuted exactly, with
witnesses of the right length, and each witness is confirmed against both
whole types. Samples of a pattern, and a string outside it, feed the
sampler. The engine is property-tested against the `regex` crate on
generated patterns, and fuzzed.

## Sampler

`Sampler::candidates(type)` generates values of a type, favouring places where
versions tend to differ: numeric bounds and neighbours of bounds, multiples of
steps, minimum and maximum lengths, objects with only required members or
with every member, extra undeclared members, duplicated array items. Every
candidate is confirmed by the validator. `Sampler::probes` returns the same
kind of values *unverified*: probes derived from one version are the natural
candidates to test against the other.

## Checker

`Checker::subsumes(left, right)` returns `Yes`, `No(witness)` or `Unknown`.

1. **Decomposition.** A left union is included if every variant is; a right
   intersection if every member is; a left intersection if any member is.
   For an exclusive left union, a variant's witness is re-validated against
   the whole union, because a value matching two variants is not a member.
2. **Right unions.** When the right variants have pairwise different JSON
   types, only the variant with the left type's JSON type can hold its values,
   which decides the question exactly. Otherwise inclusion in a single variant
   is tried; for exclusive unions this also needs disjointness from the other
   variants.
3. **Same-kind structural rules.** Numbers compare *effective* bounds (an
   integer above 0.5 starts at 1; multiples of 5 up to 12 end at 10) with
   exact rationals; strings compare lengths and textual patterns; arrays
   compare lengths, uniqueness, `contains` and every item position; objects
   compare every member, required sets, undeclared members, size bounds,
   property names and dependent requirements.
4. **Counterexample search.** When no rule proves inclusion, values of the
   left type and probes from the right type are tried; the first that the
   validators confirm is in the left type and not in the right one is the
   witness.

Nested witnesses (an item, a member) are embedded into a sample of the
enclosing type and re-verified, so the witness for an object is an object.

Recursive types are compared coinductively: a pair met again while still on
the comparison stack is assumed included. Results that relied on such an
assumption about a pair *above* them are not memoized until that pair is
decided. A step budget bounds the total work.

## Diff walk

`diff(old, new)` compares surfaces first:

| Surface | Matched by |
|---|---|
| operations | identity (`GET /pets/{}`) |
| parameters | location and name (headers case-insensitively) |
| request/response content | media type |
| responses | status key |
| response headers | name, case-insensitively |
| callbacks | name; polarity reverses inside |
| schema roots | name |

Types at matched positions are compared structurally: members by name (by
wire tag when every member on both sides has one, so a renamed Protobuf field
is one field, and a name that moved to another tag is reported as the move),
array positions by index, union variants by named type, then by JSON type,
then by kind in order. A non-union compared with a union is treated as a
one-variant union, so `string` becoming `string | null` is a variant addition
rather than a type change. Pairs already compared under the same polarity are
skipped, which terminates the walk on recursive types and reports each change
once.

Identity changes that do not alter the value set are reported as shape
changes: renamed fields, fields with new tags or aliases, enum values with
new numbers or names (numbered enum values are matched by number and by name),
changed wire encodings and RPC directions that became or stopped being
streams. Changes to exclusive groups do alter the value set and carry
evidence: moving two fields into a group loses every value that sets both.

Named types that no root reaches are compared on their own with no polarity.

## Written and read values

Every inclusion question has a producing side and an accepting side: for
lost values, old values must be accepted by the new version, and for gained
ones, new values by the old version. The producing side is validated and
sampled as *written*, the accepting side as *read*. The two differ only
for types marked `writer_exact` (Avro records and enums), whose writers emit
exactly what they declare while readers ignore extra fields, fill defaults
and accept aliases. See ADR 0010.

## Evidence and attribution

Each local change carries a direction: *narrowing* changes (a raised minimum,
a new required member) can only lose values, *widening* ones only gain, and
the rest can do either. For the node where changes occur, the checker is asked
in both needed directions. A node-level witness is attributed to a change only
if the other version rejects it for the reason the change describes (for
example `TooLong` for a lowered `maxLength`). Otherwise a witness specific to
that change is searched for among samples and probes. The result for each
direction is:

| Evidence | Meaning |
|---|---|
| `None` | proven: no such value |
| `Witness(value)` | verified value, valid in one version and not the other |
| `Unknown` | not decided |

Surface changes (an operation removed, a response added) carry no value-level
effect; the rule catalog classifies them by kind.
