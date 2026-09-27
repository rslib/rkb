---
schema: 1
id: 2b00000015
type: fact
status: active
verified: 2026-08-24
verified_how: read
tags:
  - git
  - reflog
  - gc
---

# Reflog entries expire and are eventually garbage collected

## Statement
By default, unreachable reflog entries expire after 30 days and reachable ones after 90 days; once `git gc` runs past those expiry windows, the commits they pointed at can be permanently removed if nothing else references them.

## Evidence
A reflog entry for a commit made 95 days earlier was gone after `git gc` ran, while a commit from the previous week was still recoverable through `git reflog`.
