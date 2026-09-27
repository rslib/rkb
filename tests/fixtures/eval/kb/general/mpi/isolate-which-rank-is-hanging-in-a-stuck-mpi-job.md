---
schema: 1
id: 2b00000035
type: recipe
status: active
verified: 2026-06-05
verified_how: ran
tags:
  - mpi
  - debugging
  - hang
---

# Isolate which rank is hanging in a stuck MPI job

## When to use
When an MPI job stops making progress and it isn't obvious which rank (or pair of ranks) is stuck.

## Steps
Attach to each rank's process and print a stack trace rather than guessing from logs:

```sh
for pid in $(pgrep -f ./app); do
  echo "== pid $pid =="
  gdb -p "$pid" -batch -ex 'thread apply all bt' 2>/dev/null | head -30
done
```

Ranks blocked in `MPI_Recv` or `MPI_Wait` point at which peer they are waiting on; cross-referencing that against which rank never sent the matching message narrows the search quickly.

## Evidence
Stack traces showed 31 ranks blocked in `MPI_Recv` waiting on rank 7, which had exited early on an unhandled exception without logging anything, explaining the apparent hang.
