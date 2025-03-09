# Examples

Each directory holds two versions of an API description, a README listing
the changes between them and what Faultline reports, and the command to
compare them. Run the commands from the example's directory.

| Example | Format | What it shows |
|---|---|---|
| [petstore](petstore) | OpenAPI 3.0 | Breaking, risky and compatible changes, with example values |
| [swagger](swagger) | Swagger 2.0 | A Swagger description compared with its OpenAPI 3 successor |
| [overlay](overlay) | OpenAPI and Overlay 1.0 | Internal operations removed by an overlay before comparing |
| [asyncapi](asyncapi) | AsyncAPI 2.6 and 3.0 | Sent and received messages, and a migration between versions |
| [raml](raml) | RAML 1.0 | A library with a type, a trait and a resource type |
| [smithy](smithy) | Smithy 2.0 | HTTP bindings, and a model spread over two files |
| [grpc](grpc) | Protobuf | A gRPC service across two `.proto` files |
| [grpc-gateway](grpc-gateway) | Protobuf and OpenAPI | A gRPC service checked against its HTTP/JSON description |
| [thrift](thrift) | Thrift | Wire types, exceptions and one-way calls |
| [graphql](graphql) | GraphQL SDL | Field, argument and union changes as clients see them |
| [avro](avro) | Avro schema | BACKWARD, FORWARD and FULL compatibility |
| [avro-idl](avro-idl) | Avro IDL | A protocol importing another, with messages and errors |
| [jtd](jtd) | JSON Type Definition | Closed objects and a discriminated union, as output |
| [xsd](xsd) | XML Schema | XML documents compared as objects |
| [wsdl](wsdl) | WSDL 1.1 | SOAP 1.1 and 1.2 bindings, actions, headers and faults |
| [ci](ci) | GitHub Actions, GitLab CI | Checking pull and merge requests |

The examples double as end-to-end tests: `cargo test -p faultline-cli`
compares each one and checks the findings.
