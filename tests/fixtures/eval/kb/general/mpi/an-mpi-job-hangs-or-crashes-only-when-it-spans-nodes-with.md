---
schema: 1
id: 2b00000032
type: pitfall
status: active
verified: 2026-06-02
verified_how: read
tags:
  - mpi
  - version-mismatch
  - modules
---

# An MPI job hangs or crashes only when it spans nodes with different module loads

## Symptom
A job launched across two nodes fails intermittently with:

```text
[node02][[54321,1],3][btl_openib_component.c] error obtaining device attributes
```
or simply hangs, while jobs confined to either node alone run correctly.

## Cause
One node's job environment loaded a different minor version of the MPI module than the other (for example, a stale `module load openmpi/4.1.4` on one node versus `openmpi/4.1.6` on another after a partial software update). Ranks built and launched against mismatched MPI library versions can fail to negotiate a common wire protocol.

## Fix
Pin the exact module version in the job script and verify it resolves identically on every node before launching:

```sh
module load openmpi/4.1.6
srun --mpi=pmix bash -c 'module list; ldd ./app | grep libmpi'
```

## Evidence
`ldd ./app | grep libmpi` on the two nodes showed different `libmpi.so` paths; pinning the module version identically on both nodes and resubmitting fixed the hang.
