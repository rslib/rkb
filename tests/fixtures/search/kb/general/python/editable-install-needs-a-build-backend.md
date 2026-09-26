---
schema: 1
id: 1a00000006
type: pitfall
status: active
verified: 2026-09-25
verified_how: read
tags:
  - packaging
---

# Editable install fails without a build backend

## Symptom
`pip install -e .` fails and says the directory does not appear to be a Python project.

## Cause
`pyproject.toml` has no `[build-system]` table, so pip does not know how to build the project.

## Fix
```toml
[build-system]
requires = ["setuptools>=64"]
build-backend = "setuptools.build_meta"
```

## Evidence
The editable install worked after the table was added.
