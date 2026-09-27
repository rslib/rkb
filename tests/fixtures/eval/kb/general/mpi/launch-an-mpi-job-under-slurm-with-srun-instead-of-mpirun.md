---
schema: 1
id: 2b00000033
type: recipe
status: active
verified: 2026-06-03
verified_how: ran
tags:
  - mpi
  - slurm
  - srun
---

# Launch an MPI job under Slurm with srun instead of mpirun

## When to use
Inside a Slurm allocation, when the MPI library was built with native Slurm/PMI support and the job should inherit Slurm's own resource binding instead of a second, independent launcher.

## Steps
```sh
salloc -N4 --ntasks-per-node=8
srun --mpi=pmix ./app
```

Using `srun` directly lets Slurm handle process placement and binding consistently with the rest of the allocation, rather than layering `mpirun`'s own placement logic (which reads different environment variables) on top of Slurm's.

## Evidence
Switching a job script from `mpirun -n 32 ./app` to `srun --mpi=pmix ./app` inside the same `salloc` produced the CPU binding reported by `--cpu-bind=verbose`, which matched Slurm's allocation exactly.
