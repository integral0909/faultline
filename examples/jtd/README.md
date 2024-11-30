# JSON Type Definition example

An event schema in JSON Type Definition (RFC 8927) form, in two versions.
Consumers read the events, so the schema is compared as output:

```sh
faultline diff v1/event.jtd.json v2/event.jtd.json --role output
```

| Change | Finding |
|---|---|
| `change` gained the `renamed` kind | breaking: consumers cannot read the new variant (FL1012, FL1017) |
| `attempts` is an `int8` | risky: negative values may now be sent, and generated code changes type (FL1007, FL1010) |
| `source` was added, `address` gained `country` | risky: JTD objects are closed, so validating consumers reject members they do not know (FL1002) |
| `closed.reason` gained `INACTIVE` | risky: consumers may not handle it (FL1005) |

Files ending in `.jtd.json` are read as JSON Type Definitions; name the
format with `--format jtd` for other file names.
