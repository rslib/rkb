---
schema: 1
id: 2b00000016
type: pitfall
status: active
verified: 2026-08-01
verified_how: read
tags:
  - python
  - venv
  - modulenotfounderror
---

# A package that is clearly installed still raises ModuleNotFoundError

## Symptom
```text
$ python script.py
ModuleNotFoundError: No module named 'requests'
```
but `pip show requests` reports the package is installed.

## Cause
`pip show` and `python script.py` were run against two different interpreters: a project virtualenv was activated in one terminal, but the script was run from a different terminal (or after a shell restart) where the base system Python, or a different conda environment, is active instead.

## Fix
Confirm which interpreter is actually active before installing or running anything:

```sh
which python
python -c "import sys; print(sys.executable)"
```

Activate the intended environment (`source .venv/bin/activate`) in the same shell before running the script.

## Evidence
`sys.executable` pointed at `/usr/bin/python3` instead of the project's `.venv/bin/python`; activating the venv in that shell resolved the import.
