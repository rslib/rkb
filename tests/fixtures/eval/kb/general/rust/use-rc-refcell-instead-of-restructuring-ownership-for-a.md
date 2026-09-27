---
schema: 1
id: 2b00000055
type: decision
status: active
verified: 2026-02-05
verified_how: told
tags:
  - rust
  - rc
  - refcell
  - ownership
---

# Use Rc<RefCell<>> instead of restructuring ownership for a shared mutable cache

## Context
A single-threaded command-line tool needed several parts of the code to read and occasionally update a shared in-memory cache, and a strict single-owner design kept forcing the cache to be threaded explicitly through every function signature that touched it, even ones several calls removed from actually using it.

## Decision
Wrap the cache in `Rc<RefCell<Cache>>` and clone the `Rc` wherever a reference is needed, checking borrows at runtime instead of redesigning the call graph around a single owner.

## Why
The tool is single-threaded, so `Rc`/`RefCell`'s lack of thread safety is not a real cost, and the alternative (passing `&mut Cache` through every intermediate function) would have added parameters to functions that don't otherwise care about the cache, for no benefit besides satisfying the borrow checker statically.

## Rejected options
Restructuring the call graph to pass `&mut Cache` explicitly everywhere it's needed: rejected as a large diff for no behavior change. Using `Arc<Mutex<>>` preemptively for future thread-safety: rejected as unnecessary overhead and complexity for a tool with no current multi-threading plan.
