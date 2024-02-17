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
