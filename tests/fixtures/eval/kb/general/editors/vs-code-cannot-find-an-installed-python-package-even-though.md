---
schema: 1
id: 2b00000060
type: pitfall
status: active
verified: 2026-01-05
verified_how: read
tags:
  - vscode
  - python
  - interpreter-selection
---

# VS Code cannot find an installed Python package even though pip shows it present

## Symptom
A script that imports `numpy` runs fine from a terminal inside the project's virtual environment, but running it from VS Code's "Run Python File" button shows:

```text
ModuleNotFoundError: No module named 'numpy'
```

## Cause
VS Code's Python extension runs code using whichever interpreter is selected in the status bar for that workspace, which can default to a system Python or a different environment than the one active in an integrated terminal, especially right after creating a new virtual environment that VS Code hasn't picked up yet.

## Fix
Explicitly select the project's virtual environment as the interpreter:

```text
Command Palette -> Python: Select Interpreter -> choose ./.venv/bin/python
```

## Evidence
The status bar had been showing a system Python path; selecting `.venv/bin/python` from the interpreter picker made the same "Run Python File" action resolve the `numpy` import successfully.
