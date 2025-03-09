# syntax=docker/dockerfile:1
#
# The faultline binary in a small image, for CI systems that run their
# steps in containers. The release workflow publishes it for linux/amd64
# and linux/arm64 as ghcr.io/<owner>/faultline:<version>.
#
#   docker run --rm -v "$PWD:/work" ghcr.io/integral0909/faultline:X.Y.Z \
#     diff v1/openapi.yaml v2/openapi.yaml

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p faultline-cli \
    && strip target/release/faultline

FROM alpine:3
# Certificates for remote references, when a workspace enables them.
RUN apk add --no-cache ca-certificates \
    && adduser -D -u 10001 faultline
COPY --from=build /src/target/release/faultline /usr/local/bin/faultline
USER faultline
WORKDIR /work
ENTRYPOINT ["faultline"]
CMD ["--help"]
