---
schema: 1
id: 2b00000053
type: recipe
status: active
verified: 2026-02-03
verified_how: ran
tags:
  - rust
  - cargo
  - cargo-check
---

# Get fast compiler feedback without waiting for a full build

## When to use
During active development, when the goal is catching type errors and borrow-checker complaints quickly, not producing a runnable binary yet.

## Steps
```sh
cargo install cargo-watch   # once
cargo watch -x check
```
`cargo check` runs the same analysis as a full build up through type checking and borrow checking, but skips code generation, so it finishes much faster; `cargo watch` reruns it automatically on every file save.

## Evidence
On a mid-sized crate, `cargo check` completed noticeably faster than `cargo build` after a one-line change, since it skipped LLVM codegen entirely while still catching the same compile errors.
