# XML Schema

`faultline-xml` reads XML; `faultline-xsd` reads XML Schema documents into
components and lowers them into the IR. WSDL builds on both.

## Reading XML

The reader is a small XML 1.0 parser with namespaces. Every element and
attribute keeps its span, and names are resolved to `{namespace}local` as
the document is read; `QName`-valued attributes (`type="tns:Pet"`) are
resolved later against the prefixes in scope at their element. Whitespace
between elements is dropped, and text split by comments or CDATA sections is
joined.

Document type declarations are refused (`FLE0702`). No API description
needs a DTD, and processing one is what makes XML dangerous: entities that
expand exponentially, and external entities that read files or make
requests. Without a DTD only the five predefined entities and character
references exist. Elements are read with an explicit stack, so nesting is
bounded by the configured depth limit rather than by the thread's stack.

## Reading schemas

`Reader` loads the entry document and follows `xs:include` and `xs:import`
with a `schemaLocation`, resolved against the including document and
confined to the workspace. Each document is read once, which also ends
import cycles. A document without a `targetNamespace` included into one with
a namespace takes the includer's ("chameleon" include). Imports without a
location must be satisfied by another schema of the set, as in WSDL.
`xs:redefine` and `xs:override` are read as includes, with a warning
(`FLE7207`).

Local element and attribute names are qualified according to
`elementFormDefault`, `attributeFormDefault` and `form`; global components
are always in the target namespace.

## Simple types

| XML Schema | IR |
|---|---|
| `xs:string`, `token`, `NCName`, ... | string |
| `xs:anyURI` | string, format `uri` |
| `xs:dateTime`, `date`, `time`, `duration`, `gYear`... | string with that format |
| `xs:base64Binary`, `hexBinary` | string, format `byte` or `hex` |
| `xs:boolean` | boolean |
| `xs:decimal`, `float`, `double` | number |
| `xs:integer` and its derivations | integer with the datatype's range (`xs:int` is 32-bit, `xs:unsignedByte` is 0 to 255, ...) |
| `xs:anyType`, `anySimpleType` | any value |

A restriction narrows its base: `length`, `minLength`, `maxLength`; patterns,
anchored because XML Schema patterns match whole values (several patterns in
one restriction are alternatives, and a restriction of a restricted pattern
requires both); `minInclusive` and the other bounds; `totalDigits` as an
exclusive bound of ±10ⁿ; `fractionDigits` as a step (none makes a decimal an
integer). An enumeration becomes an enum of the base's kind. A list is an
array (its length facets count items) and a union a union.

## Complex types and elements

An element of complex type is an object; see [ADR 0012](../design-decisions/0012-xml-documents-as-objects.md)
for why. Child elements are members named by their local name, optional when
they may be absent and arrays when they may repeat; attributes are
`@`-prefixed members; the text of simple or mixed content is `#text`; the
options of a `choice` form an exclusive group. Objects are closed unless
`xs:any` or `xs:anyAttribute` opens them.

An extension inherits its base's members and adds its own after them; a
restriction restates the content it keeps and inherits the attributes it
does not prohibit. Named complex types are reserved before they are lowered,
so recursive types refer to themselves.

Each global element that is not abstract is a schema root, named by its
local name (by its expanded name when two namespaces share a local name),
used in the role the configuration gives.

## Element order

The elements of a `sequence` must appear in the order it lists them, so the
object records that order. When the elements both versions have appear in
a different order, the change is reported (FL1035): breaking for documents
a service reads, and risky for documents it writes, which clients may
validate. Adding an element keeps the others' order, and `all` groups fix
no order.

## Not modelled

* Substitution groups and `xsi:type`: an abstract element is not a root, and
  derived types used through `xsi:type` are not considered.
* Identity constraints (`xs:key`, `xs:unique`, `xs:keyref`) and assertions.
* Facets on date and time values.
