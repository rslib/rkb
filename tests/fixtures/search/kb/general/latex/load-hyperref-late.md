---
schema: 1
id: 1a00000009
type: fact
status: active
verified: 2026-09-25
verified_how: read
tags:
  - hyperref
---

# Load hyperref after most other packages

## Statement
Load `hyperref` late in the preamble, after the packages it patches. Load `cleveref` after `hyperref`.

## Evidence
The hyperref and cleveref manuals both state this order.
