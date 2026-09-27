---
schema: 1
id: 2b00000017
type: pitfall
status: active
verified: 2026-08-02
verified_how: read
tags:
  - python
  - editable-install
  - modulenotfounderror
---

# Editing a package's source has no effect even though it was pip installed with -e

## Symptom
After `pip install -e .`, editing `src/mypkg/utils.py` and rerunning the script either raises `ModuleNotFoundError: No module named 'mypkg.utils'` for a newly added submodule, or keeps running the old code.

## Cause
Editable installs built with a `src/`-layout project sometimes rely on a `.pth` file or an auto-generated `__editable__` finder that was created for the package layout at install time. Adding a brand-new submodule or changing `pyproject.toml`'s package discovery doesn't update that finder until the editable install is redone.

## Fix
Reinstall the editable package after structural changes (new modules, new packages, changed `pyproject.toml` package lists):

```sh
pip install -e . --force-reinstall --no-deps
```

## Evidence
After adding a new submodule and reinstalling with `-e . --force-reinstall`, the import resolved; simply editing existing files without adding modules had not required a reinstall.
