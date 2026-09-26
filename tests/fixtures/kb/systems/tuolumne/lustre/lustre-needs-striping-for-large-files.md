---
schema: 1
id: c04e11a9f3
type: recipe
status: active
verified: 2026-09-18
verified_how: read
tags:
  - lustre
---

# Stripe a directory before writing large files on Lustre

## When to use
Before a job writes files larger than 10 GB to Lustre scratch.

## Steps
```sh
lfs setstripe -c 8 "$SCRATCH/run"
```

## Evidence
The site documentation recommends a stripe count of 8 or more for large files.
