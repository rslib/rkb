---
schema: 1
id: 2b00000048
type: fact
status: active
verified: 2026-03-03
verified_how: read
tags:
  - latex
  - latexmk
  - build
---

# latexmk reruns the compiler as many times as references require

## Statement
`latexmk -pdf` inspects the log and auxiliary files after each pass and automatically reruns `pdflatex` (and `bibtex`/`biber` when needed) until cross-references, the table of contents, and citations stabilize, instead of running a fixed number of passes.

## Evidence
A document with a table of contents, forward references, and citations needed four passes to fully resolve; `latexmk -pdf` ran exactly that many passes and then stopped, while a single manual `pdflatex` run left `??` in place of page numbers.
