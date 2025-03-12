# The language server

`faultline lsp` (crate `faultline-lsp`) serves the Language Server Protocol
over standard input and output. It shows the same findings `faultline check`
would report, while the API is being edited.

## What is compared

The workspace folder is the one the editor opens; `faultline.toml` is read
from it (from the editor's buffer when the file is open, so edits to the
configuration apply before they are saved).

* With `[[api]]` entries, each configured API is compared, with its
  overlays applied and entry patterns (`schema/*.graphql`) expanded, as
  `faultline check` does.
* Without any, each open document that is an API description on its own is
  compared. Files in the schema languages are recognized by extension:
  Protobuf, Thrift, Smithy, GraphQL, Avro, XML Schema, WSDL and JSON Type
  Definition. RAML files count when they are APIs; fragments such as
  libraries are compared through the API that uses them. JSON or YAML files
  count when their first lines declare `openapi`, `swagger`, `asyncapi`,
  `$schema` or `smithy`. Other JSON and YAML (`package.json`, CI files) stay
  quiet.

The old version is read from the base revision (`HEAD` by default; the
`--base` flag, the `base` initialization option, or the `faultline.base`
setting in `workspace/didChangeConfiguration` choose another). The new
version is the working tree, with every open document served from the
editor's text by an overlay loader layered over the workspace's file loader.
References between documents therefore see unsaved edits in any of them.

An API whose entry document does not exist in the base revision is new and
has nothing to compare with. A base revision that cannot be read (no
repository, an unknown branch) is reported once as a message, not as a
diagnostic on every file.

## When analysis runs

Messages are read on a thread of their own and handled one at a time.
Opening, changing, saving and closing documents only mark the workspace
dirty; the analysis runs once no message has arrived for 250 ms. A burst of
keystrokes therefore costs one comparison, and requests such as hover are
answered from the last results without waiting for one.

`Server::handle` takes a message and returns the replies, and
`Server::analyze` returns the notifications an analysis publishes. Neither
touches standard input or output, which is how the tests drive the server.

## What is shown

| Result | Where | Severity |
|---|---|---|
| Breaking finding | At the change in the new version | Error |
| Risky finding | At the change | Warning |
| Informational, waived or baselined finding | At the change | Hint |
| Input error (parse, reference, lowering) | At its span | Error or warning |
| Change with no place in the new version (a removal) | Top of the API's entry document, naming the old location | By severity |
| Declared version increased too little | Top of the entry document | Error |
| Expired waiver | Its line in `faultline.toml` | Error |
| Waiver matching no finding | Its line in `faultline.toml` | Hint |

Each finding's diagnostic has the rule code as its `code` and the finding id
in `data.findingId`. Files whose diagnostics disappear are published again
with an empty list, so stale results are cleared.

Waivers are reported unused only when every API could be compared: a
comparison that failed would otherwise make its waivers look unused.

## Hover and quick fixes

Hovering a finding shows the rule, its title and the first paragraph of its
explanation, the IR path of the change, an example value the change affects
when one was verified, and the finding id.

The quick fix for an active breaking or risky finding appends a waiver to
`faultline.toml`, creating the file if needed:

```toml
[[waiver]]
# FL1007 bound-changed in pets: op[GET /pets]/param[query:limit]
finding = "6059f23290bf"
reason = "<why this change is acceptable>"
expires = "2026-10-28"
```

The expiry is 30 days away and the reason is a placeholder to replace:
waivers are decisions to record, not a way to silence the editor.

## Positions

Positions use UTF-16 columns, the protocol's default encoding. Spans are
converted with the source files' line indexes, which keep checkpoints in long
lines so minified documents convert in constant time. Paths from the editor
are canonicalized against the nearest existing directory, so documents
created and never saved still belong to the workspace, and results are
published under the editor's own spelling of each URI.
