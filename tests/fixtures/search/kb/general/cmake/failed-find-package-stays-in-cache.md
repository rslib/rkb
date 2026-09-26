---
schema: 1
id: 1a00000001
type: pitfall
status: active
verified: 2026-09-25
verified_how: read
tags:
  - cmake
  - cache
---

# CMake keeps a failed find_package result in the cache

## Symptom
After installing a missing library, CMake still reports that the package is not found.

## Cause
A failed `find_package` stores `<Pkg>_DIR-NOTFOUND` in `CMakeCache.txt`. Later runs read the cache and do not search again.

## Fix
Remove the cache entry, or reconfigure from a clean cache:

```sh
cmake -U "Foo_DIR" .
cmake --fresh -B build
```

## Evidence
Reconfiguring with `--fresh` (CMake 3.24 and newer) found the library at once.
