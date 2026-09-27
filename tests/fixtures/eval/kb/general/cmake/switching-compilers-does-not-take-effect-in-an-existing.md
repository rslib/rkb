---
schema: 1
id: 2b00000002
type: pitfall
status: active
verified: 2026-09-11
verified_how: read
tags:
  - cmake
  - cache
  - compiler
---

# Switching compilers does not take effect in an existing build directory

## Symptom
Exporting `CC=clang CXX=clang++` and re-running `cmake -B build` still shows `CMAKE_CXX_COMPILER` pointing at the old GCC path in `CMakeCache.txt`, and the build keeps failing with GCC-only warnings turned into errors.

## Cause
CMake picks the compiler once, during the very first configure of a build directory, and stores it in the cache. Environment variables set afterward are ignored because the cache entry already exists and takes precedence.

## Fix
Compiler selection needs a clean build directory, not a partial cache edit:

```sh
rm -rf build
CC=clang CXX=clang++ cmake -B build
```

## Evidence
Deleting the build directory and reconfiguring picked up clang immediately; editing `CMAKE_CXX_COMPILER` in the cache by hand left stale flags from the old compiler's detection step.
