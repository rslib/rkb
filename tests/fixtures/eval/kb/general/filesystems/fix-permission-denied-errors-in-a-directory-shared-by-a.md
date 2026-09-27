---
schema: 1
id: 2b00000040
type: recipe
status: active
verified: 2026-05-05
verified_how: ran
tags:
  - filesystems
  - permissions
  - setgid
---

# Fix permission-denied errors in a directory shared by a project group

## When to use
When one teammate can read files another teammate created in a shared project directory, but cannot write new files there, or newly created files come out owned by the wrong group.

## Steps
Set the setgid bit on the shared directory so new files inherit its group, and make sure the group has write access:

```sh
chmod 2775 /shared/project
chgrp projectteam /shared/project
```

New files created underneath then belong to `projectteam` regardless of which teammate's primary group created them, as long as everyone is a member of that group.

## Evidence
After setting the setgid bit, a file created by a second teammate came out group-owned by `projectteam` instead of that teammate's personal group, and was writable by the rest of the team without an explicit `chmod`.
