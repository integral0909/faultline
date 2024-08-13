# AsyncAPI example

Two versions of an application's events. Version 1 is written in AsyncAPI
2.6; version 2 was migrated to AsyncAPI 3.0 and also changed. The migration
itself produces no findings: both versions are compared from the
application's point of view, whatever the specification version.

```sh
faultline diff v1/asyncapi.yaml v2/asyncapi.yaml
```

| Change | Finding |
|---|---|
| `PaymentReceived` (received) gained required `method` | breaking: producers' existing messages are refused (FL1002) |
| `Order` (in the sent `OrderPlaced`) lost required `note` | breaking: consumers relying on it no longer get it (FL1003) |
| `orders/{region}/placed` now also carries `OrderCancelled` | risky: consumers may not recognize it (FL2080) |
| `Order` gained optional `currency` | compatible |
| The Avro `ShipmentRequested` gained `priority` with a default | compatible: messages written without it are still read |

Received messages flow in like request bodies, sent messages flow out like
responses, and the Avro payload is compared with Avro's resolution rules.
