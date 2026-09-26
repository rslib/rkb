---
schema: 1
id: 1a00000004
type: pitfall
status: active
verified: 2026-09-25
verified_how: read
tags:
  - shallow-clone
  - ci
---

# Shallow clones break git describe

## Symptom
In CI, `git describe` fails with "fatal: No names found, cannot describe anything."

## Cause
A shallow clone made with `--depth 1` has no tags and almost no history.

## Fix
```sh
git fetch --unshallow --tags
```

## Evidence
The version step passed after the job fetched the full history.
