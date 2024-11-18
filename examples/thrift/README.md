# Thrift example

A pet service described in Thrift IDL in two versions. It inherits from a
service in an included file.

```sh
faultline diff v1/pets.thrift v2/pets.thrift
```

| Change | Finding |
|---|---|
| `Pet.age` is an `i64` | breaking: another wire type, which old readers skip (FL1031) |
| `Pet.tags` is a `set` | breaking: another wire type (FL1031) |
| `refresh` is no longer `oneway` | breaking: callers do not read the reply (FL3002) |
| `search`'s `limit` moved from id 2 to 3 | breaking: servers no longer find it (FL1026) |
| `get` can throw `Throttled` | risky: old clients fail on an exception they do not know (FL1002) |
| `Status` gained `ADOPTED` | risky: clients may not handle it (FL1005) |
| `Pet` gained an optional `owner` | compatible (FL1002) |

Each function is an operation (`op[Pets/get]`), including `version`, which
`Pets` inherits from `shared.Health`. Arguments and results are compared
field by field, by id. A reply is the result struct: `success` or one of
the declared exceptions.
