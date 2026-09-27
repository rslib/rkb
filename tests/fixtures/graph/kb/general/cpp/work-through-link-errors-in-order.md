---
schema: 1
id: 1b00000002
type: recipe
status: active
verified: 2026-09-26
verified_how: read
tags:
  - linker
---

# Work through link errors in order

## When to use
When a C++ build fails at the link step with undefined symbols.

## Steps
1. For a missing `vtable for` a class, see [the vtable lesson](undefined-vtable-means-missing-virtual.md).
2. For symbols that exist in a static library, see [library order](static-library-order-matters.md).
3. Run `nm -C` on the library to confirm the symbol is there.

## Evidence
Both checks together explained every link failure in the example project.
