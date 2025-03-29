# GraphQL example

Two versions of a small GraphQL schema:

```sh
faultline diff v1/schema.graphql v2/schema.graphql
```

Each root field is an operation (`op[query/pet]`, `op[mutation/addPet]`). Its
arguments are the request and its type is the response. Arguments of nested
fields flow from the client, so they are judged like request values. The
`@cached` directive, which clients write in queries, is compared as an
operation too (`op[directive/cached]`).

| Change | Finding |
|---|---|
| `removePet` removed | breaking (FL2001) |
| `search` gained the required argument `limit` | breaking (FL1002) |
| `NewPet` gained the required field `shelterId` | breaking (FL1002) |
| `Pet.listedAt` became nullable | breaking: clients may now receive `null` (FL1012) |
| `SearchResult` gained the member `Shelter` | breaking: clients may receive a type they have no fragment for (FL1012) |
| `@cached` can no longer be used on a `QUERY` | breaking: queries using it there stop validating (FL1006) |
| `Species` gained `BIRD` | risky in responses, compatible in `NewPet` (FL1005) |
| default of `pets(first:)` changed from 20 to 50 | risky (FL1022) |
| `Pet.owner` deprecated | informational (FL1020) |
| `Pet.owners` added, `nicknames` gained the optional argument `locale`, `@cached` gained the optional argument `scope` | compatible (FL1002) |
