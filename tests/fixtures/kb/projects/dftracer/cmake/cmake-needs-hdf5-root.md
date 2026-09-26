---
schema: 1
id: 7f3a9c2b41
type: pitfall
status: active
when:
  system: tuolumne
  hdf5: "1.12:1.14.2"
verified: 2026-09-25
verified_how: ran
tags:
  - cmake
  - hdf5
labels:
  sensitivity: internal
---

# CMake cannot find HDF5 unless HDF5_ROOT is set

## Environment
Tuolumne with the system HDF5 module and a CMake build.

## Symptom
`Could NOT find HDF5 (missing: HDF5_LIBRARIES)` during configure.

## Cause
The system HDF5 module does not export a CMake config file.

## Fix
```sh
cmake -DHDF5_ROOT=$(dirname $(dirname $(which h5cc))) ..
```

## Evidence
Configure failed without the flag. Configure passed with the flag.

## Check
```sh
cmake -S "$PROJECT_ROOT" -B . -DHDF5_ROOT=$(dirname $(dirname $(which h5cc))) | grep "Found HDF5"
```
