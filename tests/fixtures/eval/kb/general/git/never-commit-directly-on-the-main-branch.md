---
schema: 1
id: 2b00000014
type: preference
status: active
verified: 2026-08-23
verified_how: told
tags:
  - git
  - workflow
  - branching
---

# Never commit directly on the main branch

## Rule
Always create a feature branch before making commits; treat `main` as a branch that only receives merges.

## Why
A mistaken commit or force-push on a shared branch is much harder to undo cleanly than the same mistake on a disposable feature branch, and code review has nothing to attach to if changes land on `main` directly.

## How to apply
Before the first commit in a new piece of work, run `git switch -c <name>`. If a commit lands on `main` by accident, move it off immediately with `git branch <name>` followed by `git reset --hard origin/main` on `main`.
