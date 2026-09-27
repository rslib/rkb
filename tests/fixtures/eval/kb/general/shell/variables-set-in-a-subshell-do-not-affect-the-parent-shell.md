---
schema: 1
id: 2b00000025
type: fact
status: active
verified: 2026-07-14
verified_how: read
tags:
  - shell
  - subshell
  - scoping
---

# Variables set in a subshell do not affect the parent shell

## Statement
Any construct that forks a subshell — a pipeline stage, `(...)` grouping, or a backgrounded command — gets its own copy of the shell's variables; assignments made inside it are lost once the subshell exits and are never visible to the script that spawned it.

## Evidence
```sh
count=0
cat file.txt | while read -r line; do count=$((count+1)); done
echo "$count"   # still 0, because the while loop ran in a pipeline subshell
```
