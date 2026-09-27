---
schema: 1
id: 2b00000019
type: preference
status: active
verified: 2026-08-04
verified_how: told
tags:
  - python
  - venv
  - project-layout
---

# Keep each project's virtual environment inside the project directory

## Rule
Create `.venv` at the project root (`python -m venv .venv`) instead of a shared or globally named environment elsewhere on disk.

## Why
A per-project `.venv` makes the active interpreter obvious from the directory alone, avoids version conflicts between unrelated projects, and is trivially deleted and recreated when it gets into a bad state.

## How to apply
Add `.venv/` to `.gitignore`, document `python -m venv .venv && source .venv/bin/activate && pip install -r requirements.txt` in the project's setup instructions, and avoid installing project dependencies into any environment shared across projects.
