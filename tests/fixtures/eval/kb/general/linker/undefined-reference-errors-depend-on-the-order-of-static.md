---
schema: 1
id: 2b00000006
type: pitfall
status: active
verified: 2026-09-01
verified_how: read
tags:
  - linker
  - static-library
  - link-order
---

# Undefined reference errors depend on the order of static libraries on the link line

## Symptom
Linking fails with:

```text
undefined reference to `foo_init'
collect2: error: ld returned 1 exit status
```

even though `libfoo.a` is passed on the command line and clearly contains `foo_init`.

## Cause
The GNU linker resolves symbols from static archives in a single left-to-right pass and discards any object it doesn't currently need. If `libfoo.a` appears before the object file that calls `foo_init`, the linker has already moved past it by the time the reference shows up.

## Fix
List static libraries after the object files (and other libraries) that depend on them:

```sh
gcc main.o -lfoo -o app      # wrong order if main.o needs foo_init
gcc main.o -o app -lfoo      # libraries go after objects that use them
```

When libraries depend on each other circularly, wrap them in `-Wl,--start-group ... --end-group` instead of reordering by hand.

## Evidence
Moving `-lfoo` after `main.o` on the link line resolved the undefined reference without changing any source file.
