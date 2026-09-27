---
schema: 1
id: 2b00000059
type: recipe
status: active
verified: 2026-01-04
verified_how: ran
tags:
  - editorconfig
  - indentation
  - team-config
---

# Keep indentation consistent across contributors using different editors

## When to use
When a project's contributors use a mix of editors (Vim, Emacs, VS Code) and files keep arriving with inconsistent indentation because each editor's settings differ.

## Steps
Add an `.editorconfig` file at the project root:

```ini
root = true

[*]
indent_style = space
indent_size = 4
end_of_line = lf
insert_final_newline = true

[*.md]
trim_trailing_whitespace = false
```
Most popular editors read this file natively or via a small, commonly installed plugin, and apply its rules automatically without any per-contributor setup beyond installing that plugin once.

## Evidence
After adding `.editorconfig`, files newly created in VS Code, Vim, and Emacs by different contributors all came out with the same four-space indentation without anyone changing their personal editor settings.
