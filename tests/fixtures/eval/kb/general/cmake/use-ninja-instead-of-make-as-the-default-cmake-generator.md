---
schema: 1
id: 2b00000004
type: decision
status: active
verified: 2026-09-13
verified_how: told
tags:
  - cmake
  - ninja
  - build-speed
---

# Use Ninja instead of Make as the default CMake generator

## Context
Incremental rebuilds of a mid-sized C++ project (a few hundred translation units) were taking noticeably longer than expected after touching a single header.

## Decision
Configure with `-G Ninja` instead of the default Unix Makefiles generator.

## Why
Ninja's build files encode the dependency graph more precisely and its scheduler starts new jobs with less overhead per step, so incremental rebuilds after a small change finish faster than with Make on the same project.

## Rejected options
Keeping Make and parallelizing harder with `-jN`: it reduced wall-clock time for full rebuilds but did not fix the per-file overhead that dominates small incremental changes. Switching build system entirely (e.g., to Bazel): too large a migration for the benefit gained.
