---
schema: 1
id: 1b00000001
type: pitfall
status: active
verified: 2026-09-26
verified_how: read
tags:
  - cmake
  - configure
---

# A new library is not found until the CMake cache is cleared

## Symptom
You install the library that configure could not find, run CMake again, and the package is still NOTFOUND.

## Cause
The NOTFOUND value from the first run stays in `CMakeCache.txt`, and `find_package` does not search again while the cache has it.

## Fix
Unset the cached `_DIR` entry, or configure from a fresh cache:

```sh
cmake -U "Foo_DIR" .
cmake --fresh -B build
```

## Evidence
With `--fresh` the next configure found the newly installed library.
