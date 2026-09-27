---
schema: 1
id: 2b00000047
type: recipe
status: active
verified: 2026-03-02
verified_how: ran
tags:
  - latex
  - floats
  - figures
---

# Stop LaTeX from pushing every figure to the end of the document

## When to use
When figures placed with `\begin{figure}` keep drifting several pages away from where they are referenced in the text, or compilation stops with `! LaTeX Error: Too many unprocessed floats`.

## Steps
Loosen the float placement specifier and give LaTeX more options to place the figure near its source location:

```latex
\begin{figure}[htbp]
  \centering
  \includegraphics[width=0.8\linewidth]{plot}
  \caption{...}
\end{figure}
```
If floats still pile up, add `\usepackage{placeins}` and insert `\FloatBarrier` at section boundaries so floats can't drift past them.

## Evidence
Adding `\FloatBarrier` after each section kept every figure within its own section instead of several sections downstream, and resolved the `Too many unprocessed floats` error in a document with many back-to-back figures.
