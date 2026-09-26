---
schema: 1
id: 5d2e8a1c90
type: decision
status: active
verified: 2026-09-10
verified_how: told
tags:
  - regex
  - performance
---

# Use RE2 instead of std::regex in hot loops

## Context
Parsing trace lines with `std::regex` took most of the run time.

## Decision
Use RE2 for patterns that run once per trace line.

## Why
RE2 matched the same patterns about ten times faster in the profiler.

## Rejected options
- Hand-written parsers: harder to change when the trace format changes.
