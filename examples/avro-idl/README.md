# Avro IDL example

An order service described in Avro IDL (`.avdl`), in two versions. The
protocol imports its `Money` record from `money.avdl`.

```sh
faultline diff v1/orders.avdl v2/orders.avdl
```

| Change | Finding |
|---|---|
| `get` can throw `Suspended` | breaking: an old client's reader cannot resolve the new error branch (FL1012) |
| `Line.quantity` lost its default | risky: readers of data written without the field fail (FL1022) |
| `Status` gained `RETURNED` | compatible: the enum's default (`PLACED`) is what old readers see |
| `Order` gained an optional `coupon` | compatible (FL1002) |

Messages are compared as operations (`op[Orders/get]`). A message's request
is a record of its parameters, and its response is the response type or one
of the declared errors. Records and enums follow Avro's schema resolution
rules, as `.avsc` schemas do (see [examples/avro](../avro)).
