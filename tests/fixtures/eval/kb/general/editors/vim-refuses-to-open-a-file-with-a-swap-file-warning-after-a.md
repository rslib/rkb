---
schema: 1
id: 2b00000056
type: pitfall
status: active
verified: 2026-01-01
verified_how: read
tags:
  - vim
  - swap-file
  - recovery
---

# Vim refuses to open a file with a swap file warning after a crash

## Symptom
```text
E325: ATTENTION
Found a swap file by the name ".notes.md.swp"
...
(1) Another program may be editing the same file...
(2) An edit session for this file crashed.
```

## Cause
Vim writes a `.swp` file while editing and only removes it on a clean exit (`:wq`, `:q`); a crash, a killed terminal, or the machine losing power leaves the swap file behind, and the next `vim` invocation on that file finds it and assumes there might be unsaved recovery data or a conflicting editor session.

## Fix
If no other vim session is actually editing the file, recover any unsaved changes and then remove the stale swap file:

```sh
vim -r .notes.md.swp   # review recovered changes, then save
rm .notes.md.swp
```

## Evidence
`vim -r` recovered the last unsaved edit from before the crash; after saving and deleting the `.swp` file, subsequent opens of `.notes.md` no longer showed the warning.
