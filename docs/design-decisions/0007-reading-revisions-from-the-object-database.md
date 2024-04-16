# 0007. Reading revisions from the object database

* **Status:** Accepted
* **Date:** 2024-04-16

## Context

The most common place to run Faultline is a pull request: compare the API as
the branch leaves it with the API as it was on the target branch. That needs
the old version's files, including every file its references reach, not just
the entry document.

Two details make this harder than it looks. The target branch keeps moving,
so comparing with its tip blames the pull request for changes merged since it
branched. And the files come from a revision the user did not write, so they
are as untrusted as the working tree.

## Alternatives

* **A second checkout** (`git worktree add`, or copying files around in the
  CI script). Works with any tool, but every user writes the same fragile
  script, reference resolution sees two unrelated directories, and a shallow
  clone or a stale worktree fails in confusing ways.
* **Run the `git` command.** `git show REV:path` per file. Adds a runtime
  dependency on a git binary, spawns a process per document and needs care
  with argument quoting and repository configuration (hooks, `core.fsmonitor`,
  filters) that can execute programs.
* **Read the object database in-process** with a git library, behind the same
  `Loader` interface the filesystem uses.

For the library: `git2` (bindings to libgit2, a C library) or `gix`
(pure Rust).

## Decision

`faultline-vcs` reads revisions with `gix`, using only revision parsing,
merge-base computation and object lookup, with default features off.
`GitLoader` implements `Loader`, so every front end resolves references in a
revision exactly as it does on disk:

* the workspace root's position in the working tree selects the subtree that
  becomes the workspace, and URIs outside it are refused as before;
* the object header's size is checked against the file-size limit before the
  blob is read;
* symbolic links are refused rather than followed, and submodules are not
  entered.

`faultline check` compares each configured API with the **merge base** of the
base revision and the head by default; `--no-merge-base` compares with the
base revision itself.

## Reasoning

Keeping revisions behind `Loader` means there is one resolution path and one
set of sandbox rules to review. `gix` keeps the build free of C code and
`unsafe`-heavy bindings, and reading objects in-process never runs hooks,
filters or other configured programs.

Following symbolic links inside a revision would need a second, tree-based
resolver with its own escape checks; refusing them is simpler to get right.
Legitimate API descriptions rarely rely on links.

Comparing with the merge base matches what reviewers see in a pull request's
diff, and does not fail a branch because of changes someone else merged.

## Trade-offs

* `gix` adds a sizeable dependency tree, audited by `cargo-deny` like the rest.
* A shallow clone may not contain the merge base; CI needs `fetch-depth: 0`
  (or enough history), and the error names the revisions that failed.
* Descriptions that use symbolic links must be checked from the working tree
  (`faultline diff` without revisions).

## Consequences

* Git-related diagnostics have their own codes: `FLE0903` (repository),
  `FLE0904` (revision or merge base) and `FLE0905` (an API that does not exist
  in the base yet, reported as a note).
* Source names carry the revision (`main:api/openapi.yaml`), and reporters
  that annotate checked-out files prefer working-tree locations over
  revision locations, whose line numbers may differ.
