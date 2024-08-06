# 0011. A small, bounded HTTP server for the registry

* **Status:** Accepted
* **Date:** 2024-08-06

## Context

The registry accepts API descriptions over HTTP, checks them, and stores
the compatible ones. Its clients are CI jobs and command-line tools: each
sends one request, reads one response and disconnects. Request rates are
low, and each request costs far more in lowering and comparison than in
HTTP handling. The server faces untrusted networks, usually behind a proxy
that terminates TLS.

## Alternatives

* **An asynchronous framework** (hyper with tokio, or one built on them).
  Mature and fast, but it brings an async runtime and dozens of crates into
  a tool that is otherwise synchronous. The work per request is CPU-bound
  comparison, which an async runtime would have to move to blocking threads
  anyway.
* **A small synchronous server crate.** Fewer dependencies, but the
  candidates are lightly maintained, and their defaults (keep-alive, chunked
  bodies, unbounded headers) would each need to be checked and overridden.
* **A server of our own over `std::net`**, implementing only what the
  registry's clients use.

## Decision

The registry has its own HTTP/1.1 server of a few hundred lines. It serves
one request per connection (`Connection: close`) on a fixed pool of
threads. Bodies are framed only by `Content-Length`: requests with
`Transfer-Encoding` are refused, and conflicting lengths are an error. The
request line and headers share a byte budget, bodies have a size limit
checked before reading, and sockets have read and write timeouts.
`Expect: 100-continue` is honoured, since common clients wait for it. A
handler that panics produces a 500 response and leaves the worker running.

## Consequences

* No async runtime or server framework in the dependency tree.
* Every limit is explicit and tested, and the request parser is a fuzz
  target.
* No connection reuse and no HTTP/2: each request pays a connection setup,
  which is negligible next to the comparison it asks for. A proxy in front
  can offer HTTP/2 and keep-alive to clients.
* TLS is the proxy's job. The server listens on localhost by default so an
  unencrypted listener is not exposed by accident.
