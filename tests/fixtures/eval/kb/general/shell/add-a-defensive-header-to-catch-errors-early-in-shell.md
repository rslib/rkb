---
schema: 1
id: 2b00000023
type: recipe
status: active
verified: 2026-07-12
verified_how: ran
tags:
  - shell
  - bash
  - error-handling
---

# Add a defensive header to catch errors early in shell scripts

## When to use
At the top of any bash script that should stop immediately on a failing command, an unset variable, or a failure hidden inside a pipeline.

## Steps
```sh
#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'
```

`-e` exits on the first failing command, `-u` treats unset variables as errors, `-o pipefail` makes a pipeline fail if any stage fails (not just the last one), and the `IFS` change avoids accidental word splitting on spaces.

## Evidence
With `set -o pipefail` added, `grep missing file.txt | sort` correctly aborted the script when `grep` found nothing, instead of silently continuing because `sort` (the last command in the pipe) exited zero.
