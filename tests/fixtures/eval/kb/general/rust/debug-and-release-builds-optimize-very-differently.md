---
schema: 1
id: 2b00000054
type: fact
status: active
verified: 2026-02-04
verified_how: read
tags:
  - rust
  - cargo
  - build-profiles
---

# Debug and release builds optimize very differently

## Statement
`cargo build` (the debug profile) compiles with `opt-level = 0` and includes debug assertions and overflow checks by default, while `cargo build --release` compiles with `opt-level = 3`, strips debug assertions, and disables overflow checks unless explicitly re-enabled in `Cargo.toml`; the two can differ by an order of magnitude in runtime speed on numeric code.

## Evidence
A tight numeric loop ran roughly 20x slower under `cargo run` (debug) than under `cargo run --release` on the same input, consistent with debug assertions and disabled optimizations rather than any bug.
