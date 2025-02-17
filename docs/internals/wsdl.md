# WSDL

`faultline-wsdl` reads WSDL 1.1 and 2.0 descriptions and lowers their
operations into the IR, with the schemas they carry lowered by the XML Schema front end
([xsd.md](xsd.md)) into the same type graph.

## Reading

The reader collects messages and parts, port types and their operations
(input, output, faults, documentation), bindings and services. Schemas in
`wsdl:types` are handed to the XML Schema reader as they are, and
`wsdl:import` loads either another description, merged into this one, or a
schema document. Imports are resolved inside the workspace, and each
document is read once. SOAP 1.1 and SOAP 1.2 bindings are understood; other
bindings (HTTP GET/POST, MIME) contribute no protocol details.

## WSDL 2.0

WSDL 2.0 descriptions are read into the same components, so both versions
are lowered, and compared, alike:

* An `interface` is a port type. `extends` brings in the operations of the
  interfaces it names.
* An operation's `input`, `output` and faults name elements directly. Each
  becomes a message with one part holding the element. `#any` (or
  `#other`) is any content, and `#none` is no content.
* An in-only operation has no output, and is one-way.
* A SOAP binding is SOAP 1.2 unless `wsoap:version="1.1"`. It is document
  style and has each operation's `wsoap:action`.
* HTTP bindings contribute no protocol details, as in WSDL 1.1.

A description in the namespace of a pre-recommendation draft of WSDL 2.0
is reported (`FLE7402`).

## Operations

Each operation of each port type is an operation named by port type and
operation (`op[PetPortType/GetPet]`), whether or not a service exposes it:
the port type is the interface clients are generated from.

| WSDL | IR |
|---|---|
| input message | request body |
| output message | `200` response |
| faults | `500` response: a union of one-member objects named after each fault |
| SOAP binding | a content entry: `text/xml` (1.1) or `application/soap+xml` (1.2) |
| SOAP 1.1 `soapAction` | a required `SOAPAction` header allowing only the action |
| SOAP 1.2 `soapAction` | the `action` parameter of the media type |
| `soap:header` | a required header parameter (input) or response header (output) |

The body follows the binding's style. A document-style body with one part
naming an element is that element; with several parts, an object of them.
An RPC-style body is an element named after the operation (and the
operation plus `Response` for the output) holding one member per part, so
switching between the styles changes the body as it changes the messages
on the wire. `parts` on `soap:body` selects the parts in the body, and parts
bound as headers are not in it.

## Consequences for findings

* Dropping a binding removes a media type every client of that binding
  uses: breaking.
* Changing the action breaks clients that send the old one, in the header
  for SOAP 1.1 and in the media type for SOAP 1.2.
* A new fault is a new variant of the fault response, which clients with
  exhaustive fault handling may not expect; a removed fault is harmless.
* Changes inside parts are type changes of the schemas: a new required
  element in a request breaks callers, a new enumeration value in a response
  may surprise them.

## Not modelled

* Service addresses: moving an endpoint is a deployment change.
* SOAP encoding (`use="encoded"`) is compared as if the parts were literal
  (warning `FLE7406`).
* `wsdl:fault` bindings and `soap:headerfault`.
* WS-Policy, WS-Addressing and other extensions.
