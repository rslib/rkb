---
schema: 1
id: 2b00000034
type: fact
status: active
verified: 2026-06-04
verified_how: read
tags:
  - mpi
  - barrier
  - synchronization
---

# MPI_Barrier synchronizes entry, not exit timing

## Statement
`MPI_Barrier` guarantees that no rank returns from the call until every rank has entered it, but it does not guarantee that all ranks exit at the same wall-clock time; a rank on a slower or more loaded node can still return noticeably later than others.

## Evidence
Timestamping the return of `MPI_Barrier` across ranks in a profiling run showed a spread of several milliseconds between the fastest and slowest rank, even though the call is a correctness synchronization point, not a timing one.
