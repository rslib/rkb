---
schema: 1
id: 1b00000003
type: recipe
status: superseded
superseded_by: 1a00000007
verified: 2026-09-26
verified_how: read
tags:
  - pip
---

# Install packages with pip --user

## When to use
When you need a Python package and cannot write to the system site-packages.

## Steps
```sh
pip install --user requests
```

## Evidence
The package installed into the home directory.

## Why superseded
`pip --user` picks whichever pip is first on `PATH`; running pip through the interpreter installs for the right Python.
