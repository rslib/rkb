---
schema: 1
id: 2b00000020
type: fact
status: active
verified: 2026-08-05
verified_how: read
tags:
  - python
  - pip
  - interpreter
---

# pip always installs into the interpreter that invoked it, not whatever python is first on PATH

## Statement
Running `pip install X` installs into the same interpreter that the `pip` command itself belongs to, which is not necessarily the interpreter that `python` or `python3` resolves to on `PATH`; these can silently diverge when multiple Pythons are installed.

## Evidence
`pip --version` showed it belonged to `/usr/local/bin/python3.9`, while `python3` on `PATH` resolved to `/usr/bin/python3.11`; packages installed with `pip install` were invisible to scripts run with `python3`.
