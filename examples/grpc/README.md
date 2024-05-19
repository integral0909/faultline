# gRPC example

Two versions of a small gRPC service across two `.proto` files, with changes
that matter to the binary wire format and some that only matter to names:

```sh
faultline diff v1/pets/v1/pets.proto v2/pets/v1/pets.proto
```

`pets.proto` imports `pets/v1/species.proto`, found through the `v1/`
directory as `protoc -I v1` would find it, and
`google/protobuf/timestamp.proto`, which is built in.

| Change | Finding |
|---|---|
| `DeletePet` removed | breaking (FL2001) |
| `ListPets` returns a stream | breaking (FL3001) |
| `page_size` changed from `int32` to `sint32` | breaking: the zigzag encoding misreads varint data (FL1031) |
| `SPECIES_DOG` renumbered from 2 to 4 | breaking: old data decodes as an unknown value (FL1029) |
| `age_months` changed from `int32` to `int64` | risky in responses: old clients truncate values above 2³¹−1 (FL1007) |
| field 2 renamed from `name` to `display_name` | risky: binary data is unaffected, JSON and generated code break (FL1025) |
| `owner_email` and `owner_phone` moved into a `oneof` | compatible in responses, where receivers only ever see fewer combinations; breaking wherever clients send the message (FL1028) |
