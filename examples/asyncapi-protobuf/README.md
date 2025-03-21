# AsyncAPI with Protobuf payloads

An order service that publishes `OrderPlaced` events to Kafka, described in
AsyncAPI 3.0 with the payload written in Protobuf. The payload's source
imports `Money` from `money.proto`, beside the AsyncAPI document.

```sh
faultline diff v1/asyncapi.yaml v2/asyncapi.yaml
```

| Change | Finding |
|---|---|
| `Line.sku` became `uint64` | breaking: consumers can no longer read the field's old values (FL1001) |
| `Line.quantity` became `int64` | risky: consumers reading it as `int32` truncate larger values (FL1007, FL1010) |
| field 1 renamed from `order_id` to `id` | risky: the binary encoding is unchanged, but JSON uses the new name (FL1025, FL1027) |
| `customer_id` removed and its number reserved | risky: consumers that relied on it no longer receive it (FL1003) |
| `coupon` added | compatible: consumers skip fields they do not know (FL1002) |

The payload is the first top-level message of its source; fields are matched
by number, as for `.proto` files (see [examples/grpc](../grpc)). The
operation sends the message, so the changes are judged by what existing
consumers can read.
