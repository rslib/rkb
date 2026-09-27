---
schema: 1
id: 2b00000024
type: pitfall
status: active
verified: 2026-07-13
verified_how: read
tags:
  - shell
  - quoting
  - word-splitting
---

# An unquoted variable silently splits into multiple arguments

## Symptom
```sh
file="My Report.pdf"
cp $file /backup/
cp: cannot stat 'My': No such file or directory
cp: cannot stat 'Report.pdf': No such file or directory
```

## Cause
An unquoted variable expansion undergoes word splitting on `IFS` (spaces, tabs, newlines by default) and filename globbing before the command runs, turning one filename with a space into two separate arguments.

## Fix
Quote every variable expansion that can contain a path or arbitrary text:

```sh
cp "$file" /backup/
```

## Evidence
Quoting `$file` made `cp` receive it as a single argument, and the file with a space in its name copied correctly.
