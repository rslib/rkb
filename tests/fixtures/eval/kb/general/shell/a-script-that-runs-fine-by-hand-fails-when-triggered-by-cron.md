---
schema: 1
id: 2b00000021
type: pitfall
status: active
verified: 2026-07-10
verified_how: read
tags:
  - shell
  - cron
  - path
---

# A script that runs fine by hand fails when triggered by cron

## Symptom
Running `$HOME/backup.sh` from a terminal works, but the cron log (or a mailed error) shows:

```text
$HOME/backup.sh: line 4: aws: command not found
```

## Cause
Cron runs jobs with a minimal environment: a bare `PATH` (often just `/usr/bin:/bin`) and none of the exports set up by `.bashrc` or `.bash_profile` for an interactive login shell. Any tool installed under a user-specific or third-party directory (`~/.local/bin`, `/opt/aws-cli`) is invisible to the cron job.

## Fix
Make the script self-contained instead of relying on the interactive environment:

```sh
#!/bin/sh
PATH=/usr/bin:/bin:/opt/aws-cli/bin:$HOME/.local/bin
export PATH
```

or call tools by their absolute path inside the script.

## Evidence
Adding an explicit `PATH` assignment at the top of the script made the cron-triggered run succeed identically to the interactive run.
