---
schema: 1
id: 1a00000011
type: fact
status: active
verified: 2026-09-25
verified_how: read
tags:
  - binding
  - slurm
---

# srun and mpirun bind processes differently

## Statement
Under Slurm, `srun` applies the CPU binding of Slurm, while `mpirun` uses the binding rules of the MPI library, so the same job can place ranks on different cores.

## Evidence
Compare the layout with `srun --cpu-bind=verbose` and the verbose binding report of the MPI launcher.
