---
schema: 1
id: 1a00000007
type: preference
status: active
verified: 2026-09-25
verified_how: read
tags:
  - pip
---

# Run pip through the interpreter

## Rule
Call `python -m pip` instead of `pip`.

## Why
A bare `pip` can belong to another Python on `PATH`, so packages end up in the wrong environment.

## How to apply
In docs and scripts, write `python -m pip install <package>`.
