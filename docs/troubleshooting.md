# Troubleshooting

Diagnostics carry a code (`error[FLE1004]`). This page covers the ones people
most often run into, grouped by symptom. Exit code 2 means Faultline could not
read the inputs or the configuration, so it made no judgement about
compatibility; exit code 3 is a bug, please report it with the inputs.

## `faultline check`

**`no base revision; pass --base`.** Outside GitHub Actions pull request
builds there is no default base. Pass the branch the change will merge into,
usually as a remote-tracking branch: `--base origin/main`.

**`FLE0904` cannot resolve revision / no common ancestor.** The base is not in
the clone. CI checkouts are often shallow and only contain the head commit;
fetch the history (`actions/checkout` with `fetch-depth: 0`, or
`git fetch --unshallow origin`) and the base branch. "No common ancestor"
means the merge base is missing, which also points to a shallow clone, or the
branches really have unrelated histories; `--no-merge-base` compares with
the base directly.

**`FLE0903` not inside a git repository.** `check`, `--old-rev` and
`--new-rev` read from the repository containing the workspace root. Run from
inside the working tree, or pass `--root`.

**`FLE0905` the API is new.** The entry document does not exist in the base
revision. This is a note, not an error: there is nothing to compare yet.
If the file was moved, the old path is not followed; the first check after a
move reports the API as new.

**`… is a symbolic link in main; links are not followed`.** Files read from
revisions must be regular files. Replace the link with the file, or compare
the working tree without a revision.

**Changes that are on `main` show up as reversions.** You are comparing with
the tip of the base branch (`--no-merge-base`, or `diff --old-rev main`).
`check` without `--no-merge-base` compares with the commit the branch started
from.

## References

**`FLE1004` / `FLE2003` outside the workspace root.** A `$ref` climbs above
the workspace root (`../../shared/pet.yaml`), or a symbolic link points out of
it. Widen the root with `--root` or `[workspace] root` so it contains every
referenced file.

**`FLE1005` remote references are disabled.** A `$ref` names an `https://`
document. Enable `[network] allow_remote` and list the host in
`allowed_hosts` (see [configuration.md](configuration.md#remote-references)),
or vendor the document into the workspace.

**`FLE1005` refused by the network policy.** The reference is to a host not
on the allowlist, uses plain HTTP, embeds credentials, or its host resolves
to a private, loopback or otherwise non-public address. The last one is
deliberate even for allowlisted names: internal schema servers must be
vendored or checked out. HTTP proxies are not used for the same reason.

**`FLE1006` could not load.** The file is missing, unreadable, not UTF-8
(files with a UTF-16 or UTF-32 byte-order mark are named as such; convert
them), larger than the size limit, or a remote fetch failed; the message says
which.

**`FLE1003` unresolved reference.** The document loads but the fragment
(`#/components/schemas/Pet`, `#anchor`) names nothing in it. Pointers are
case-sensitive and use `~1` for `/` and `~0` for `~`.

## Formats

**`FLE2010` Swagger 2.0.** Only OpenAPI 3.0 and 3.1 are read. Convert the
description to OpenAPI 3 first.

**The format is misdetected.** Documents with a top-level `openapi` (or
`swagger`) field are OpenAPI; everything else is JSON Schema. Pass `--format`
or set `format` in the `[[api]]` entry to override.

**`FLE1007` unknown dialect.** The `$schema` URI is not one of the JSON
Schema drafts Faultline knows (draft-04 to 2020-12). The schema is read with
the dialect that applied before it (the default, 2020-12, at a document
root); the warning names which.

**`FLE1010` approximated.** `unevaluatedProperties` or `unevaluatedItems`
depends on which branches of a conditional or union evaluated a member, and
Faultline treats such members as always evaluated. Changes that only matter
for some instances of that schema may be missed; the warning marks where.

## Protobuf

**`FLE3002` cannot find import.** Imports are searched from the entry file's
directory upwards to the workspace root; the note lists the directories
tried. Make the workspace root (`--root`) contain the import root, or move
the entry so that an ancestor of it is the import root.

**`FLE3003` cannot find type / not imported.** Names resolve as in `protoc`:
from the innermost scope outwards, where the first scope containing the first
component of the name decides. A type from another file must be imported
directly or re-exported with `import public`.

**`FLE3001` syntax errors in valid files.** Faultline reads proto2, proto3
and editions. Please report files `protoc` accepts and Faultline does not.

## GraphQL

**`FLE4006` the schema has no query root type.** Define `type Query`, or
name the root types in a `schema { query: … }` definition.

**`FLE4007` operations in a schema document.** The entry must be a schema
(type definitions), not a document of queries.

**Types defined in several files.** One file per API is read. Combine split
schemas into one file first.

## Findings

**A change is reported at a different path than expected.** A type shared by
several operations is compared once per direction and reported at the first
position that reaches it. `faultline ir dump` shows the model and the
direction of every type.

**A deliberate break needs to ship.** Add a waiver with an expiry date for its
finding id, or record a baseline when adopting Faultline on an API with
known issues (see [getting-started.md](getting-started.md#adopting-on-an-existing-api)).

**Exit code 4 with no failing findings.** A waiver in `faultline.toml`
expired. Renew it with a new date and reason, or remove it.
