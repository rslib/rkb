---
schema: 1
id: 2b00000010
type: fact
status: active
verified: 2026-09-05
verified_how: read
tags:
  - linker
  - static-library
  - start-group
---

# Linking a static library twice can still fail on circular dependencies

## Statement
A single left-to-right link line only makes one pass over each static library, so two libraries that call into each other (A needs a symbol from B, B needs one from A) can fail to resolve no matter which order they are listed in, unless one is repeated or both are wrapped in `-Wl,--start-group ... -Wl,--end-group`.

## Evidence
Listing `-la -lb` failed with an undefined reference from `libb.a` into `liba.a`; wrapping both in `--start-group`/`--end-group` let the linker revisit each archive until all references resolved.
