---
schema: 1
id: 1a00000013
type: pitfall
status: active
verified: 2026-09-25
verified_how: read
tags:
  - linker
  - static-libraries
---

# Static library order matters to the GNU linker

## Symptom
The link reports undefined symbols that exist in a static library on the command line.

## Cause
The GNU linker resolves symbols left to right and only takes members of a static library that earlier inputs need.

## Fix
List libraries after the objects that use them, or group them:

```sh
cc main.o -Wl,--start-group -la -lb -Wl,--end-group
```

## Evidence
The link passed with the group around the two libraries.
