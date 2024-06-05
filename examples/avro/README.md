# Avro example

Two versions of an event schema, compared in each compatibility mode.
`--role` chooses the mode: `input` is BACKWARD (consumers on the new schema
read events written with the old one), `output` is FORWARD (consumers still
on the old schema read new events) and `both` is FULL.

```sh
faultline diff v1/pet.avsc v2/pet.avsc --role input    # BACKWARD
faultline diff v1/pet.avsc v2/pet.avsc --role output   # FORWARD
```

| Change | BACKWARD | FORWARD |
|---|---|---|
| `weightGrams` added without a default | breaking: old events lack it (FL1002) | compatible: old readers ignore it |
| `vaccinated` added with a default | compatible | compatible |
| `shelter` removed | compatible: new readers ignore it | breaking: old readers require it (FL1003) |
| `name` renamed to `displayName`, alias `name` | risky: new readers match it through the alias, generated code changes (FL1025) | risky (FL1025); old readers do not know the new name |
| `ageMonths` from `int` to `long` | compatible: a promotion | risky: old `int` readers cannot hold every `long` (FL1007) |
| `Species` gained `BIRD` | compatible | risky: old readers without an enum default cannot read it (FL1005) |
