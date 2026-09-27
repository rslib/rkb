---
schema: 1
id: 2b00000003
type: recipe
status: active
verified: 2026-09-12
verified_how: ran
tags:
  - cmake
  - clangd
  - tooling
---

# Generate compile_commands.json so editors can resolve project headers

## When to use
When an editor's language server marks every `#include` in a CMake project as unresolved because it does not know the project's include paths.

## Steps
```sh
cmake -DCMAKE_EXPORT_COMPILE_COMMANDS=ON -B build
ln -sf build/compile_commands.json .
```

Most language servers look for `compile_commands.json` starting at the project root and walking up, so a symlink at the top level is enough.

## Evidence
After adding the symlink, the editor's language server started resolving third-party include paths and stopped flagging valid includes as errors.
