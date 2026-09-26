---
schema: 1
id: 1a00000010
type: recipe
status: active
verified: 2026-09-25
verified_how: read
tags:
  - latexmk
---

# Rebuild a LaTeX document on every save

## When to use
While writing, when you want a continuous preview of the PDF without running commands by hand.

## Steps
```sh
latexmk -pdf -pvc main.tex
```

## Evidence
latexmk rebuilt the PDF each time the source was saved.
