# WSDL example

A pet service described in WSDL 1.1, with SOAP 1.1 and SOAP 1.2 bindings, in
two versions.

```sh
faultline diff v1/pets.wsdl v2/pets.wsdl
```

| Change | Finding |
|---|---|
| `GetPet` requires a `locale` element | breaking: existing requests lack it (FL1002) |
| `CountPets`' SOAP 1.1 action is `urn:example:pets/v2/CountPets` | breaking: clients send the old `SOAPAction` (FL1006) |
| `Status` gained `adopted` | risky: clients may not handle it in responses (FL1005) |
| `CountPets`' `limit` part is an `xs:long` | risky: a different declared format (FL1010), although every old value is still accepted |

Operations are named by port type and operation (`op[PetPortType/GetPet]`).
Request and response bodies are the SOAP bodies, one entry per SOAP binding
(`text/xml` for SOAP 1.1, `application/soap+xml` with the action for SOAP
1.2), and faults are the `500` response.
