---
schema: 1
id: 2b00000013
type: recipe
status: active
verified: 2026-08-22
verified_how: ran
tags:
  - git
  - reflog
  - branch
---

# Restore a branch that was deleted by mistake

## When to use
After `git branch -D feature-x` (or a force-push that discarded it) when the branch's commits are still needed.

## Steps
```sh
git reflog show --all | grep feature-x
# or, if the branch name doesn't appear, search recent HEAD moves
git reflog
git branch feature-x <sha-of-last-known-commit>
```

## Evidence
`git reflog` retained the branch's last commit SHA for weeks after the delete; recreating the branch at that SHA brought back the full commit history.
