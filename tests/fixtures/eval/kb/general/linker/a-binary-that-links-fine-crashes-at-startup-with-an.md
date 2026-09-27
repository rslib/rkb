---
schema: 1
id: 2b00000008
type: pitfall
status: active
verified: 2026-09-03
verified_how: read
tags:
  - linker
  - shared-library
  - rpath
---

# A binary that links fine crashes at startup with an unresolved shared library symbol

## Symptom
The build finishes without errors, but running the binary prints:

```text
./app: symbol lookup error: ./app: undefined symbol: bar_compute
```

## Cause
The linker only checks that a symbol exists somewhere among the shared libraries given at link time; it does not embed a strict version requirement by default. At runtime the dynamic loader picked up a different, older copy of `libbar.so` from the system search path that does not export `bar_compute`, because the binary has no `-rpath` pointing at the library actually used to link it.

## Fix
Embed the library's location in the binary, or point the loader at it explicitly:

```sh
gcc main.o -L/opt/bar/lib -lbar -Wl,-rpath,/opt/bar/lib -o app
# or, at run time
LD_LIBRARY_PATH=/opt/bar/lib ./app
```

## Evidence
Setting `LD_LIBRARY_PATH` to the build library's directory made the binary run; `ldd ./app` then showed `libbar.so` resolving to the intended path instead of the system one.
