---
schema: 1
id: 2b00000012
type: pitfall
status: active
verified: 2026-08-21
verified_how: read
tags:
  - git
  - rebase
  - reflog
---

# Commits go missing after an interactive rebase

## Symptom
After running `git rebase -i HEAD~5` and dropping what looked like an empty commit, three unrelated commits are also gone from `git log`.

## Cause
Dropping a line in an interactive rebase removes that commit and rewrites every commit after it with new SHAs; if the todo list was edited incorrectly (a line deleted instead of marked `drop`, or reordered past a dependent commit), later commits can end up silently squashed or omitted from the resulting history.

## Fix
The rebase left the old chain intact in the reflog before it rewrote history:

```sh
git reflog
git reset --hard HEAD@{5}   # the entry just before the rebase started
```

Then redo the rebase more carefully, changing only the intended line.

## Evidence
`git reflog` showed an entry labeled `rebase (start)`; resetting to the commit just before it restored all five original commits.
