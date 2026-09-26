---
schema: 1
id: 1a00000016
type: fact
status: active
verified: 2026-09-25
verified_how: read
tags:
  - lustre
---

# lfs getstripe shows the layout of a file

## Statement
`lfs getstripe <file>` prints the stripe count, stripe size and the storage targets of a file.

## Evidence
The output matched the layout set by `lfs setstripe`.
