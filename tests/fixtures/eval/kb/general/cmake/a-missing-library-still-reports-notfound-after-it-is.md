---
schema: 1
id: 2b00000001
type: pitfall
status: active
verified: 2026-09-10
verified_how: read
tags:
  - cmake
  - find_package
---

# A missing library still reports NOTFOUND after it is installed

## Symptom
After `apt install libfoo-dev`, rerunning `cmake -B build` still prints:

```text
CMake Error at CMakeLists.txt:5 (find_package):
  Could not find a package configuration file provided by "Foo"
```

## Cause
The first failed `find_package(Foo)` call wrote `Foo_DIR-NOTFOUND` into `CMakeCache.txt`. Subsequent `cmake` invocations trust the cached value and never search the filesystem again, even though the package is now present.

## Fix
Clear only the stale cache entry, or start over:

```sh
cmake -U 'Foo_DIR' -B build
# or, from scratch
rm -rf build && cmake -B build
```

## Evidence
Removing the cached `Foo_DIR` entry with `-U` let CMake re-run `find_package` and locate the newly installed config file on the next configure.
