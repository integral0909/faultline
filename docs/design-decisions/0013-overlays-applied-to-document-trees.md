# 0013. Overlays applied to document trees, per version

* **Status:** Accepted
* **Date:** 2024-12-13

## Context

Many teams publish an OpenAPI description through an overlay: one source
description with internal operations and extensions, and an overlay that
removes them or adds documentation for the public. Consumers see the
overlaid description, so that is what should be compared, and a change
that the overlay removes should not be reported.

Overlays select their targets with JSONPath and change the JSON or YAML
tree. They can be applied before Faultline reads the description, inside
it, or after the description has been lowered.

## Alternatives

* **Leave it to the pipeline**: run an overlay tool, write the result to a
  file, and compare the files. This needs no code, but findings then point
  into generated files, a pull request check has to regenerate both
  versions, and the old version needs the overlay as it was then.
* **Apply overlays to the IR**: map JSONPath targets onto operations and
  types. Overlays are defined on the document, so a target such as
  `$.paths.*[?@['x-internal']]` has no meaning in the IR, and updates can
  add any member, including ones the IR does not model.
* **Apply overlays to the document tree**, between parsing and lowering.

## Decision

Overlays are applied to the entry document's tree after it is parsed and
before it is lowered. They are read through the same loader as the entry,
so each version is read with the overlays of its own workspace or revision.
Values an update brings keep their spans in the overlay file.

An overlay that is missing from a revision is a note, not an error. That
version is compared without the overlay, because that is the description
its consumers saw.

## Consequences

* Findings about content an overlay added point into the overlay, and a
  pull request that changes only an overlay is checked like any other
  change.
* Overlays change the entry document only. Documents it references through
  `$ref` are compared as they are.
* Formats that are not JSON or YAML documents (Protobuf, Thrift, GraphQL,
  XML) cannot take overlays, and giving one is an error.
* Faultline carries its own JSONPath implementation (RFC 9535), including
  filters and the standard functions, so that overlays behave the same in
  every environment.
