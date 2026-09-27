---
schema: 1
id: 2b00000018
type: recipe
status: active
verified: 2026-08-03
verified_how: ran
tags:
  - python
  - packaging
  - dependencies
---

# Pin exact dependency versions with a lock file

## When to use
When a project's `requirements.txt` lists loose version ranges and a fresh install on another machine pulls in a newer, incompatible release of a dependency.

## Steps
```sh
pip install pip-tools
pip-compile requirements.in -o requirements.txt
pip install -r requirements.txt
```

`requirements.txt` then pins every transitive dependency to an exact, tested version; regenerate it with `pip-compile --upgrade` when a controlled upgrade is wanted.

## Evidence
A fresh clone installed from the compiled `requirements.txt` reproduced the exact dependency versions used in CI, instead of picking up an unrelated dependency's newer major release.
