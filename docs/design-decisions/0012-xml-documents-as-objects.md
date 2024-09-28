# 0012. XML documents compared as objects

* **Status:** Accepted
* **Date:** 2024-09-28

## Context

XML Schema describes documents made of elements, attributes and text, in a
particular order, while Faultline's IR describes values: objects with named
members, arrays, scalars. WSDL services exchange such documents, so XML
Schema must be compared in the IR, or the comparison machinery (inclusion,
witnesses, direction) would have to be rebuilt for a second model.

## Alternatives

* **An XML-specific type model** with elements, attributes, text and order,
  and its own inclusion checker. Exact, but duplicates the checker and the
  rules, and a WSDL service with JSON bindings could not be compared across.
* **Automata over element sequences**, as XML Schema's formal semantics
  does. Precise for order and occurrence, but a large piece of work whose
  results are hard to explain in terms of fields.
* **Documents as objects**: members for child elements and attributes,
  arrays for repetition, exclusive groups for choices.

## Decision

An element of complex type is an object whose members are its child
elements (by local name) and its attributes (`@name`), with `#text` for
character content. `minOccurs="0"` makes a member optional, `maxOccurs`
above one makes it an array with the same bounds, a `choice` is an
exclusive group, and `nillable` adds `null`. Objects are closed unless the
schema has wildcards. Simple types become the IR kind their values denote.

## Consequences

* Every rule, the witness search and reporting work for XML unchanged, and
  findings name positions as `schema[order].line[].price`.
* Element order is not compared. Reordering a sequence breaks strict
  validators and is not reported.
* An element of simple type becomes an object when it gains its first
  attribute, which is reported as a type change although documents without
  the attribute stay valid.
* Namespaces of child elements are not part of member names: two children
  with the same local name in different namespaces share a member.
