---
schema: 1
id: b7e24a9d15
type: pitfall
status: active
verified: 2026-09-22
verified_how: ran
tags:
  - export
  - gateway
queries:
  - "export: error E17: gateway rejected the request"
  - export.py fails with E17
---

# export.py fails with error E17 unless EXPORT_PROFILE is set

## Symptom
`export: error E17: gateway rejected the request` when running `python3 export.py`.

## Cause
The gateway accepts only requests that carry the profile name in `EXPORT_PROFILE`. The script does not say so.

## Fix
Run it with the profile: `EXPORT_PROFILE=eu2-gw python3 export.py`.

## Evidence
`python3 export.py` printed error E17. `EXPORT_PROFILE=eu2-gw python3 export.py` printed `exported 3 files`.
