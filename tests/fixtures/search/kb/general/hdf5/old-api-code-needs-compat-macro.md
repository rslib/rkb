---
schema: 1
id: 1a00000014
type: pitfall
status: active
when:
  hdf5: "1.10:"
verified: 2026-09-25
verified_how: read
tags:
  - hdf5
  - api
---

# Old HDF5 API code needs a compatibility macro

## Symptom
Code written for the HDF5 1.8 API fails to compile against a newer HDF5, with errors in calls such as `H5Dopen` and `H5Gcreate`.

## Cause
Newer releases changed the default signatures of these functions.

## Fix
```sh
cc -DH5_USE_18_API -c reader.c
```

## Evidence
The code compiled with the macro against HDF5 1.12.
