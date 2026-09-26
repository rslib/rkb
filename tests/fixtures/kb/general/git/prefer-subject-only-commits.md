---
schema: 1
id: 9a4c7e2b10
type: preference
status: active
verified: 2026-09-01
verified_how: told
tags:
  - git
---

# Prefer subject-only commit messages

## Rule
Write a commit message as one Conventional Commits subject line.

## Why
The user reads history with `git log --oneline`.

## How to apply
Add a body only when the change needs a reason that the diff does not show.
