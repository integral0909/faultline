# The schema registry

`faultline registry serve` runs a registry that stores versions of API
descriptions and refuses versions that would break the clients of earlier
ones. Any format Faultline reads can be registered: OpenAPI, JSON Schema,
Protobuf, GraphQL and Avro, including descriptions spread over many files.

## Running a registry

```sh
export FAULTLINE_REGISTRY_TOKEN="<a long random value>"
faultline registry serve --dir /var/lib/faultline-registry
```

| Flag | Default | |
|---|---|---|
| `--dir` | required | Where versions are stored (created if needed) |
| `--listen` | `127.0.0.1:8080` | Address to listen on |
| `--fail-on` | `breaking` | Findings of this severity refuse a version, for subjects without settings |
| `--transitive` | off | Check against every earlier version, for subjects without settings |
| `--threads` | 8 | Requests served at once |
| `--max-body-bytes` | 16 MiB | Largest request accepted |

The server speaks plain HTTP and listens on localhost by default: put a
proxy that terminates TLS in front of it to serve other hosts. When
`FAULTLINE_REGISTRY_TOKEN` is set, registering versions and changing
settings require it as a bearer token; reading and dry-run checks do not.
One server process serves a directory.

Storage is plain files: `subjects/<subject>/v<n>.json` holds version `n`
with all of its files, and `subjects/<subject>/config.json` the subject's
settings. Versions are never modified after they are written, so backing up
the directory at any moment captures a consistent registry.

## Registering versions

```sh
export FAULTLINE_REGISTRY_TOKEN="<the registry's token>"
faultline registry push https://registry.example.com petstore api/openapi.yaml
```

`push` lowers the description locally, collects the entry document and every
workspace file it references, and sends them. The registry compares the new
version with the latest one (or, for transitive subjects, with every one)
and stores it only if no finding fails the subject's settings:

```text
$ faultline registry push https://registry.example.com petstore api/openapi.yaml
refused (HTTP 409): the version is incompatible with version 3
  * breaking FL1007 bound-changed: the largest allowed value of `limit` dropped from 100 to 50
    at op[GET /pets]/param[query:limit]
```

Findings marked `*` are the ones that failed. Sending a version identical
to the latest registers nothing and succeeds, so pushing from every CI run
of the main branch is safe.

`faultline registry check` runs the same comparison without registering,
and needs no token: use it in pull requests to learn early whether a change
will be accepted. Both commands take `--root`, `--format`, `--role` and
`--json` (print the registry's response as JSON).

| Exit code | Meaning |
|---|---|
| 0 | Registered, identical to the latest, or (for `check`) compatible |
| 1 | Refused because of findings |
| 2 | Invalid description, bad request, authentication failure, or no registry |

Descriptions with remote references cannot be registered: the registry reads
only the files it is sent.

## HTTP interface

All bodies are JSON (`Content-Type: application/json`). Errors are
`{"error": "..."}` with a 4xx or 5xx status.

| Method | Path | |
|---|---|---|
| `GET` | `/health` | `{"status": "ok"}` |
| `GET` | `/subjects` | `{"subjects": [...]}` |
| `GET` | `/subjects/{s}/versions` | `{"subject", "versions": [1, 2, ...]}` |
| `POST` | `/subjects/{s}/versions` | Register a version |
| `GET` | `/subjects/{s}/versions/{v}` | Version `v` (or `latest`) with its files |
| `GET` | `/subjects/{s}/versions/{v}/files/{path}` | One file's text |
| `POST` | `/subjects/{s}/check` | Check without registering |
| `GET` | `/subjects/{s}/config` | `{"subject", "fail_on", "transitive"}` |
| `PUT` | `/subjects/{s}/config` | Change settings; fields not given keep their values |

A version is sent as a bundle:

```json
{
  "entry": "api/openapi.yaml",
  "format": "openapi",
  "role": "both",
  "files": {
    "api/openapi.yaml": "openapi: 3.1.0\n...",
    "schemas/pet.json": "{...}"
  }
}
```

`format` and `role` are optional, as on the command line. File paths are
relative and `/`-separated, without `.` or `..` segments.

Registering answers `201 Created` with a `Location` header for a new
version, `200` when the bundle is identical to the latest version, `409`
when findings refuse it, and `422` when the description has errors. Every
answer that compared something carries `report`: the same JSON report
`faultline diff --output json` prints (see [configuration.md](configuration.md)
for its fields). Findings name the subject as their API; in transitive
checks, comparisons with versions other than the latest name it
`{subject}@v{n}`.

Subject names are 1 to 128 letters, digits, `.`, `_` and `-`, not starting
with `.`. Names differing only in case are refused, since they would share a
directory on case-insensitive filesystems.

Registered files can be referenced directly:
`https://registry.example.com/subjects/petstore/versions/3/files/schemas/pet.json`
serves the schema as `application/json`, so other descriptions can `$ref`
a pinned version when remote references are enabled in their
`[network]` settings.
