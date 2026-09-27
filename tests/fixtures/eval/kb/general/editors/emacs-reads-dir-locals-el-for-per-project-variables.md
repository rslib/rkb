---
schema: 1
id: 2b00000058
type: fact
status: active
verified: 2026-01-03
verified_how: read
tags:
  - emacs
  - dir-locals
  - project-config
---

# Emacs reads .dir-locals.el for per-project variables

## Statement
Emacs looks for a file named `.dir-locals.el` in a buffer's directory (and its parents, walking up) and applies any variable settings it finds to buffers under that directory, which lets a project set indentation width, coding style, or mode-specific variables without every contributor changing their personal Emacs configuration.

## Evidence
Placing `((python-mode . ((python-indent-offset . 2))))` in a project's `.dir-locals.el` made every Python buffer opened under that directory use two-space indentation, while buffers outside the project kept the user's own default.
