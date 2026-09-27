---
schema: 1
id: 2b00000011
type: pitfall
status: active
verified: 2026-08-20
verified_how: read
tags:
  - git
  - checkout
  - reflog
---

# A commit seems to disappear after switching branches

## Symptom
After committing on a branch and then running `git checkout main`, `git log` on the original branch no longer shows the commit, and `git branch` doesn't list the branch it was made on either.

## Cause
The commit was made while `HEAD` was detached (checking out a commit or tag directly instead of a branch), so it was never attached to any branch ref. Once `HEAD` moved elsewhere, nothing pointed at that commit anymore, though the commit object itself is still in the repository.

## Fix
Recover it from the reflog, which records every position `HEAD` has held:

```sh
git reflog
git branch recovered-work <sha-from-reflog>
```

## Evidence
`git reflog` listed the detached commit's SHA a few entries back; creating a branch at that SHA restored full access to the commit and its history.
