# XML Schema example

Two versions of an order document's schema. XML documents are compared as
objects: child elements are members, attributes are `@`-prefixed members,
repeated elements are arrays.

```sh
faultline diff v1/order.xsd v2/order.xsd --role input    # documents others send us
faultline diff v1/order.xsd v2/order.xsd --role output   # documents we send
```

With `--role input`:

| Change | Finding |
|---|---|
| `Currency` no longer allows `SEK` | breaking: orders in SEK are rejected (FL1006) |
| `Line` gained the required `price` | breaking: existing orders lack it (FL1002) |
| `line` limited to 100 occurrences | at least risky: larger orders are rejected (FL1007) |
| `CustomerId` accepts 6 to 8 digits | compatible: every old id still matches |
