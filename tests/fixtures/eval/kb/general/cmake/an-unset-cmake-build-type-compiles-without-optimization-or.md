---
schema: 1
id: 2b00000005
type: fact
status: active
verified: 2026-09-14
verified_how: read
tags:
  - cmake
  - build-type
  - optimization
---

# An unset CMAKE_BUILD_TYPE compiles without optimization or debug flags

## Statement
If `CMAKE_BUILD_TYPE` is left empty, CMake's default compiler flags do not include any optimization or debug-info flags at all (no `-O2`, no `-g`), unlike explicit `Release` or `Debug` builds.

## Evidence
Comparing `cmake --build build -- -v` output for an unset build type against `-DCMAKE_BUILD_TYPE=Release` showed the Release build passing `-O3 -DNDEBUG`, while the unset build passed neither.
