# 0010. Writer and reader views of a type

* **Status:** Accepted
* **Date:** 2024-06-02

## Context

Until Avro, each type in the IR described one set of values, and inclusion
between an old and a new set decided compatibility. Avro separates producing
data from consuming it. A writer emits every field of a record and nothing
else. A reader ignores fields it does not know, fills in defaults for fields
it misses, matches renamed fields through aliases, and may map unknown enum
symbols to a default. With a single set per schema, one of the two has to
be wrong:

* with the reader's set (open, defaults optional), a new writer "may send
  anything", and every removal looks risky;
* with the writer's set (closed, everything required), adding a defaulted
  field breaks old data, which Avro explicitly allows.

## Alternatives

* **Two types per schema**, one for each view. This doubles the IR and every
  front end for one format's sake.
* **Rules specific to Avro** in the classifier. This duplicates value-level
  reasoning outside the checker.
* **A flag on the type that relates the two views**, with the checker
  reading its left side as written and its right side as read.

## Decision

`ObjectType::writer_exact` and `EnumType::writer_exact` mark types whose
writers produce exactly the declared fields or symbols. The validator and
sampler take a mode. Written values of an exact object have every field
and no other, and written values of an exact enum are its listed symbols.
The checker and the differ's attribution read the side values come from as
written, and the side that must accept them as read. That side is left for
lost values and right for gained ones, since each is an inclusion "values
produced ⊆ values accepted".

Readers honour field aliases. Rejections list every missing required field,
so a finding can be attributed even when another change causes the first
failure.

## Consequences

* Avro's BACKWARD, FORWARD and FULL modes are the `input`, `output` and
  `both` roles, with no Avro-specific classification.
* Types without the flag, which is every other format, behave as before.
  One view is the other.
* The alias and missing-field improvements apply to every format.
  Protobuf's JSON names are aliases too.
