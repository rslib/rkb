---
schema: 1
id: 1a00000002
type: recipe
status: active
verified: 2026-09-25
verified_how: read
tags:
  - clangd
  - cmake
---

# Export compile_commands.json for clangd

## When to use
When clangd cannot find the headers of a CMake project and marks every include as an error.

## Steps
```sh
cmake -DCMAKE_EXPORT_COMPILE_COMMANDS=ON -B build
ln -s build/compile_commands.json .
```

## Evidence
clangd resolved the include paths after `compile_commands.json` was at the project root.
