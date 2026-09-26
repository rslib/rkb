---
schema: 1
id: 1a00000003
type: recipe
status: active
verified: 2026-09-25
verified_how: read
tags:
  - rebase
---

# Rebase with local edits using autostash

## When to use
When you want to pull or rebase but have uncommitted changes that you are not ready to commit.

## Steps
```sh
git pull --rebase --autostash
git config --global rebase.autoStash true
```

## Evidence
Git stashed the work tree, rebased, and applied the changes again.
