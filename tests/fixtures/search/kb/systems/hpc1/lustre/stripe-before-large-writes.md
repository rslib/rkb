---
schema: 1
id: 1a00000015
type: recipe
status: active
verified: 2026-09-25
verified_how: read
tags:
  - lustre
  - striping
---

# Stripe a directory before writing large files

## When to use
Before a job writes files larger than several GB to the Lustre scratch file system.

## Steps
```sh
lfs setstripe -c 8 "$SCRATCH/run"
```

## Evidence
Write bandwidth grew with the stripe count in a test job.
