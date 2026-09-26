---
schema: 1
id: 1a00000005
type: recipe
status: active
verified: 2026-09-25
verified_how: read
tags:
  - rebase
---

# Squash fixups with autosquash

## When to use
When a review asks for small corrections to earlier commits in a branch.

## Steps
```sh
git commit --fixup=<sha>
git rebase -i --autosquash main
```

## Evidence
Each fixup commit moved next to its target and was squashed into it.
