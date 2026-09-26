---
schema: 1
id: 1a00000008
type: fact
status: active
verified: 2026-09-25
verified_how: read
tags:
  - venv
  - fish
---

# Fish shell needs activate.fish for a virtual environment

## Statement
In the fish shell, `source .venv/bin/activate` fails with a syntax error. Use `source .venv/bin/activate.fish` instead.

## Evidence
The venv module ships a separate activate script for fish.
