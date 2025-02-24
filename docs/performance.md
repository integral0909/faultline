# Performance

Faultline runs in CI on every pull request, so a comparison should take
seconds even for very large APIs, and hostile input must not make it take
minutes or crash.

## Measurements

`cargo bench -p faultline` compares generated OpenAPI descriptions:
operations over object schemas that reference each other in a chain, with a
fixed share of them changed. It times parsing, lowering, comparison and
classification in-process. On a laptop (Apple M-series, one release build):

| Operations | YAML size | Findings | Time |
|---:|---:|---:|---:|
| 500 | 0.2 MB | 27 | 38 ms |
| 2,000 | 0.8 MB | 100 | 149 ms |
| 8,000 | 3.3 MB | 398 | 609 ms |

Time grows linearly with the size of the description. The numbers are
indicative; run the benchmark to compare changes on your machine.

`cargo bench -p faultline --bench formats` does the same for the front ends
that read interface definition languages, on descriptions of one shape:
record types that refer to each other in a chain, an operation per type,
and a twentieth of the types changed. For 4,000 operations:

| Format | Size | Findings | Time |
|---|---:|---:|---:|
| Protobuf | 0.46 MB | 1,200 | 78 ms |
| Thrift | 0.51 MB | 1,600 | 94 ms |
| Smithy | 0.59 MB | 1,200 | 122 ms |
| GraphQL | 0.32 MB | 792 | 130 ms |

Each grows linearly from 250 operations up.

## Where the time goes

* **Parsing** is linear. Minified documents, a single multi-megabyte line,
  report positions in constant time per diagnostic, because the line index
  keeps column checkpoints every 4 KB within long lines.
* **Lowering** walks each schema once, from a worklist.
* **Comparison** visits each pair of types once per polarity. While walking,
  positions are built as shared-tail trails, so descending a level is
  constant time and a full path is only materialized where a change is
  recorded. Operations are matched through maps, not pairwise.
* **Inclusion checks** are memoized per pair of types and bounded by a step
  budget.

The two versions of an API are lowered in parallel, and `faultline check`
compares several APIs on as many threads as there are cores. The output does
not depend on scheduling: results are assembled in configuration order, and
diagnostics are sorted by file name and position.

## Limits

Beyond these limits, work stops and the result says so ("unknown", a
diagnostic) instead of taking unbounded time or stack:

| Stage | Limit |
|---|---|
| Documents | file size, nesting depth and node count (`Limits`) |
| Protobuf imports | 10,000 files |
| Comparison walk | 256 levels from the root that reached a type; deeper named types are compared from their own root |
| Inclusion checks | 256 nested comparisons, 200,000 steps |
| Regular expressions | quantifier bounds of 1,000, 20,000 NFA states, 5,000 DFA states and a budget on their total size |

The regular expression and front-end limits are exercised by fuzz targets
and by tests that time worst cases.
