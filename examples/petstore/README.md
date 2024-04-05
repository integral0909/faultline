# Pet store example

Two versions of a small OpenAPI 3.0 description with a mix of breaking,
risky and compatible changes:

```sh
faultline diff v1/openapi.yaml v2/openapi.yaml
```

What changed between the versions, and how Faultline judges it:

| Change | Finding |
|---|---|
| `DELETE /pets/{petId}` removed | breaking (FL2001) |
| `limit` maximum lowered from 100 to 50 | breaking: clients sending 100 are rejected (FL1007) |
| required query parameter `species` added | breaking (FL2010) |
| `NewPet.name` maxLength lowered from 64 to 32 | breaking in the request body, compatible in responses (FL1007) |
| `tag` became nullable | breaking in responses (clients may receive `null`), compatible in requests (FL1012) |
| `status` gained the value `adopted` | risky in responses (FL1005) |
| response `429` added | risky (FL2040) |
