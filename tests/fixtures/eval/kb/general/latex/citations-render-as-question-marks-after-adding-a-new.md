---
schema: 1
id: 2b00000049
type: pitfall
status: active
verified: 2026-03-04
verified_how: read
tags:
  - latex
  - bibtex
  - citations
---

# Citations render as question marks after adding a new reference

## Symptom
A newly added `\cite{newkey}` compiles to `[?]` in the PDF, even though `newkey` is present in the `.bib` file and other citations render correctly.

## Cause
Adding a citation changes what the bibliography step needs to produce, but `pdflatex` alone does not invoke `bibtex`; if only `pdflatex` was rerun after the edit (skipping the `bibtex` step), the `.bbl` file used for citation formatting is still the one generated before the new key existed.

## Fix
Rerun the full sequence, not just `pdflatex`, after adding or changing citations:

```sh
pdflatex paper.tex
bibtex paper
pdflatex paper.tex
pdflatex paper.tex
```
or simply use `latexmk -pdf paper.tex`, which detects this automatically.

## Evidence
Rerunning `bibtex paper` followed by two more `pdflatex` passes replaced the `[?]` with the correct citation number.
