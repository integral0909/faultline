# AsyncAPI

`faultline-asyncapi` lowers AsyncAPI 2.0–2.6 and 3.0 descriptions. The two
versions organize the same information differently: in 2.x operations live
inside channels and are named from the *client's* point of view (`publish`
means clients publish to the application), while in 3.0 operations are
top-level, point at channels, and say what the *application* does
(`action: send` or `receive`). Faultline lowers both to the application's
point of view, so that a migration from 2.6 to 3.0 is not itself a change.

## Operations

| 2.x | 3.0 | IR |
|---|---|---|
| `channels.<address>.publish` | `action: receive` | `op[receive <address>]` in `operations` |
| `channels.<address>.subscribe` | `action: send` | `op[send <address>]` in the webhook `sent` |
| channel key | `channels.<id>.address` (the id when `null`) | the address template, parameters normalized to `{}` |
| `message` (or `message.oneOf`) | `messages`, or all of the channel's | the operation's messages |
| — | `reply` | the operation's `response[default]` |

Received messages are request bodies: values flow in, and producers are the
clients whose messages must still be accepted. Sent messages are the API's
own requests, like OpenAPI webhooks: values flow out, and consumers are the
clients who must still understand them. Placing send operations in a
webhook gives them, and their channel parameters, the outward direction
without special cases in the comparison.

Two 3.0 operations with the same action on the same channel are merged:
together they carry the union of their messages.

An operation whose address changed but whose parameters, messages and
replies did not is reported as moved (FL2004), as for HTTP paths.

## Messages

Each message is keyed by its name: the `name` field, else (2.x) its
`messageId`, else the name it is referenced by (`#/components/messages/X`,
`#/channels/c/messages/X`), else its position (`message1`). In paths it
appears as `message[OrderPlaced]`.

A message is compared as an open object with three members:

* `contentType`: a single-valued enum holding the message's content type
  (or `defaultContentType`, or `application/json`), so a change of encoding
  is a change of the values both sides accept;
* `headers`: the headers schema, when declared;
* `payload`: the payload schema; a message without one carries any value.

Adding a message to a received operation is compatible; removing one breaks
producers still sending it (FL2081). Adding a message to a sent operation is
risky, since consumers may not recognize it (FL2080); removing one is risky,
since consumers stop receiving it.

## Payload formats

| `schemaFormat` | Lowered as |
|---|---|
| absent, `application/vnd.aai.asyncapi…` | JSON Schema draft-07 (the AsyncAPI Schema Object) |
| `application/schema+json;version=…` (or `+yaml`) | JSON Schema in the named draft |
| `application/vnd.oai.openapi…` | OpenAPI 3.0 Schema Object (3.1: JSON Schema 2020-12) |
| `application/vnd.apache.avro…` | Avro, by the Avro front end |
| anything else | any value, with warning FLE6005 |

In 2.x `schemaFormat` is a message field and applies to the payload; in 3.0
a payload (or headers) may be a Multi Format Schema Object
(`{schemaFormat, schema}`).

Avro payloads are lowered by the Avro front end into an API of their own,
whose type graph is then grafted into the AsyncAPI description's arena
(`TypeArena::graft` renumbers every reference). Avro's resolution rules
therefore apply unchanged: in a received message a new field needs a
default, in a sent one removing a field needs the old readers to have had a
default for it. Named Avro types become named types of the API. An Avro
payload may also be a reference to an `.avsc` file.

## Traits

Message traits are applied. The fields that matter to the comparison are
`name` (the message's identity), `contentType`, `schemaFormat` (2.x) and
`headers`. In 2.x traits are merge patches applied in order, so the last
trait that sets a field overrides the message; in 3.0 the message's own
fields take precedence and traits fill in the rest. Header schemas from the
message and its traits are compared as their `allOf`, which is how a merge
patch combines header schemas that constrain different members. Of
operation traits only the 2.x `operationId` affects the comparison.

## Not modelled

* Servers, protocol bindings, correlation ids and security schemes are not
  compared.
* 3.0 channels that no operation uses have no direction and are not
  compared.
