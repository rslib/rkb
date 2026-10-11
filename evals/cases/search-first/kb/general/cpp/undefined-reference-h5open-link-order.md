---
schema: 1
id: 3c81d0e6a2
type: pitfall
status: active
verified: 2026-09-20
verified_how: ran
tags:
  - hdf5
  - linking
queries:
  - undefined reference to `H5open`
  - linker cannot find hdf5 symbols
---

# Undefined reference to H5open when linking against HDF5

## Symptom
`undefined reference to `H5open`` at link time, although `-lhdf5` is on the command line.

## Cause
The linker reads libraries left to right. `-lhdf5` came before the object files, so nothing used it yet.

## Fix
Put `-lhdf5` after the object files: `g++ main.o -o app -lhdf5`.

## Evidence
The link failed with the flag first and passed with the flag last.
