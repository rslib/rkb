---
schema: 1
id: 2b00000050
type: preference
status: active
verified: 2026-03-05
verified_how: told
tags:
  - latex
  - biblatex
  - biber
---

# Prefer biblatex with biber over plain bibtex for new documents

## Rule
Start new LaTeX documents with `\usepackage[backend=biber]{biblatex}` instead of the classic `\bibliographystyle`/`\bibliography` bibtex workflow, unless a venue's template mandates bibtex specifically.

## Why
Biber (biblatex's backend) handles Unicode author names and non-ASCII `.bib` entries correctly, supports multiple independent bibliographies in one document, and its citation style is customizable without editing a `.bst` file, which bibtex effectively requires for any nontrivial formatting change.

## How to apply
Set `\usepackage[style=numeric,backend=biber]{biblatex}` and `\addbibresource{refs.bib}` in the preamble, and configure the build tool (or `latexmk`) to call `biber` instead of `bibtex` for that document.
