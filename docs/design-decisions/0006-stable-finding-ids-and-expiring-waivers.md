# 0006. Stable finding ids and expiring waivers

* **Status:** Accepted
* **Date:** 2024-04-05

## Context

Teams need to accept specific findings: a break negotiated with every
consumer, a false positive, a legacy problem being worked on. Acceptance has
to survive unrelated edits to the description, and it must not quietly
become permanent — the most common failure of suppression mechanisms is that
nobody ever revisits them.

## Alternatives

* **Inline suppression comments** in the API description. Tied to lines that
  move; impossible in JSON; clutters the contract with tooling concerns.
* **Suppression by rule and path** in configuration. Stable, but coarse: it
  accepts every future finding at that position too.
* **Suppression by finding id**, where the id is derived from what the
  finding *is* rather than where it is printed.

For lifetime: permanent suppressions, optional expiry, or mandatory expiry.

## Decision

* A finding id is 12 hexadecimal digits of the FNV-1a hash of the API name,
  rule code, `IrPath`, and a kind-specific detail (the enum value, the member
  name, the facet). Spans, line numbers and file names are not part of it.
* Waivers reference finding ids and must carry a reason and an expiry date.
  After expiry the finding counts again and the stale waiver fails the check
  with its own exit code until it is renewed or removed.
* Baselines, for adopting Faultline on an existing API, are also keyed by
  finding id but do not expire; they are replaced wholesale.
* Severity overrides by rule and path glob remain available for policy
  decisions that are meant to be permanent ("responses of internal endpoints
  are informational").

## Reasoning

`IrPath` already names positions independently of layout, so an id built from
it is stable across reformatting, file moves and `$ref` restructuring, and
changes exactly when the finding does. The kind-specific detail separates two
findings at one position (two removed enum values).

Mandatory expiry turns accepted risk into a scheduled decision. A distinct
exit code for expired waivers lets CI distinguish "a new break" from "a
decision is due".

## Trade-offs

* Renaming an API or restructuring paths changes ids and invalidates waivers.
* The hash function is now a compatibility commitment; changing it would
  invalidate every waiver and baseline in use. A pinned test guards it.
* Expiry adds friction by design.

## Consequences

* Reports show finding ids everywhere a user might copy one.
* SARIF output carries the id as a partial fingerprint, so code-scanning UIs
  track findings across runs.
