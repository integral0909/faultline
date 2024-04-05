# Architecture decision records

Each record captures one significant decision: the context that forced it, the
alternatives considered, what was chosen and why, and what it costs. Records are
immutable once accepted; a later decision that changes course gets a new record
that marks the old one as superseded.

New records start from [template.md](template.md) and take the next number.

| # | Decision | Status |
|---|---|---|
| [0001](0001-rust-and-workspace-layout.md) | Rust, and a workspace of focused crates | Accepted |
| [0002](0002-document-parsing.md) | Own spanned JSON reader; YAML through an event parser | Accepted |
| [0003](0003-unified-ir-with-computed-polarity.md) | A unified IR with computed polarity | Accepted |
| [0004](0004-sandboxed-offline-reference-resolution.md) | Sandboxed, offline-by-default reference resolution | Accepted |
| [0005](0005-three-valued-inclusion-with-verified-witnesses.md) | Three-valued inclusion with verified witnesses | Accepted |
| [0006](0006-stable-finding-ids-and-expiring-waivers.md) | Stable finding ids and expiring waivers | Accepted |
