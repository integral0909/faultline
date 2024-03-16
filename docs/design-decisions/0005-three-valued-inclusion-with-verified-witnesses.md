# 0005. Three-valued inclusion with verified witnesses

* **Status:** Accepted
* **Date:** 2024-03-16

## Context

Deciding whether a schema change breaks consumers is deciding inclusion
between sets of JSON values: does the new request schema accept everything
the old one did? For the full JSON Schema language this is undecidable
(`not`, `oneOf` and regular expressions interact in arbitrary ways), and even
the decidable fragments have corner cases that are easy to get wrong: adding a
`oneOf` variant can *reject* values that now match twice; a lower bound of 0.5
on an integer means 1; `multipleOf: 0.1` is not a floating-point question.

A compatibility checker that is wrong in either direction does damage.
False "breaking" findings teach teams to ignore the tool; false "safe"
results let real breaks through.

## Alternatives

* **Syntactic rules only.** "maxLength decreased" is breaking, and so on.
  Simple, but blind to interactions between keywords and to composition.
* **A complete decision procedure** for a restricted schema language,
  rejecting anything outside it. Precise where it applies, useless on real
  schemas that use `allOf` inheritance, `oneOf` and patterns.
* **Sound proof rules plus verified counterexample search, with an explicit
  unknown.** Prove inclusion where the constructs allow it; otherwise search
  for a value that belongs to one type and not the other, and verify it with
  an exact validator; if neither succeeds, say so.

## Decision

Inclusion answers are `Yes`, `No(witness)` or `Unknown`:

* `Yes` comes only from rules that are sound for the constructs involved:
  bound comparisons after normalizing to effective integer and step
  boundaries, member-by-member and item-by-item recursion, union and
  intersection decomposition, and JSON-type disjointness.
* `No` always carries a witness that the validator has confirmed is a member
  of one type and not of the other. Witnesses for nested positions are
  embedded in a value of the enclosing type and re-verified.
* Recursive types are compared coinductively. A result that depends on an
  assumption about a pair higher up the stack is not memoized until that pair
  is decided.
* Numbers use exact rational arithmetic; overflow yields `Unknown`.

## Reasoning

The asymmetry is deliberate. A witness is a concrete, checkable claim, so
counterexample search can be heuristic (sampling boundary values of both
versions) without ever producing a wrong `No`. A `Yes` has no such
certificate, so it is only issued by rules whose soundness can be argued
locally. Everything else is `Unknown`, which the policy layer can report as
"risky" instead of guessing.

Witnesses also make findings actionable: "`{"name": "aaaa…"}` (33
characters) was accepted and is now rejected" explains a finding better than
any rule text.

## Trade-offs

* Some changes that a complete procedure could decide come out `Unknown`,
  notably regular-expression patterns until the regex layer lands, and
  intersections that only a merged view could compare.
* Counterexample search costs time. It runs only for nodes that changed, is
  memoized per type pair, and is capped by a step budget.
* The validator must be exact wherever it answers. It is checked against an
  independent JSON Schema implementation with property tests.

## Consequences

* `faultline-subsume` provides the validator, sampler and checker;
  `faultline-diff` attributes witnesses to individual changes by the reason
  the other version rejects them.
* Every new construct needs a validator case (exact or `Unknown`) before it
  can take part in comparison.
* Rules never need to re-derive semantics: they classify changes by kind,
  polarity and evidence.
