# RAML example

A pet API described in RAML 1.0 in two versions. Both versions use a library
(`lib/common.raml`) that provides a `Status` type, a `paged` trait and a
`collection` resource type.

```sh
faultline diff v1/pets.raml v2/pets.raml
```

| Change | Finding |
|---|---|
| `DELETE /pets/{petId}` was removed | breaking: operation removed (FL2001) |
| The library's `paged` trait lowered `limit`'s maximum to 50 | breaking: 100 was accepted and is now rejected (FL1007) |
| `species` is now required | breaking: existing requests may lack it (FL2012) |
| The library's `Status` gained `adopted` | risky: clients may not handle it in responses (FL1005) |
| `Pet` gained an optional `owner` | compatible (FL1002) |

Findings about the library point into `lib/common.raml`. The operations
`/pets` gets from `collection`, and the parameters its methods get from
`paged`, are compared as if they had been written out in place.
