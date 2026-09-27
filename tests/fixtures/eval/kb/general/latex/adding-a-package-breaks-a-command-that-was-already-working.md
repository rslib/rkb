---
schema: 1
id: 2b00000046
type: pitfall
status: active
verified: 2026-03-01
verified_how: read
tags:
  - latex
  - undefined-control-sequence
  - packages
---

# Adding a package breaks a command that was already working

## Symptom
After adding `\usepackage{newpkg}` near the top of the preamble, compiling produces:

```text
! Undefined control sequence.
l.42 \mycommand
```
for a command that compiled fine before.

## Cause
The new package redefines or internally uses a command name that collides with one already defined by another package or by the document's own preamble; depending on load order, the later package's definition silently overwrites the earlier one, sometimes with a different argument signature or under a different internal name.

## Fix
Check the new package's documentation for name clashes, and if needed, load it before the package that defines the command actually being used, or rename the conflicting custom command:

```latex
\newcommand{\myowncommand}{...}  % renamed away from the colliding name
```

## Evidence
Swapping the load order of the two packages restored `\mycommand`'s original definition and the document compiled without the undefined control sequence error.
