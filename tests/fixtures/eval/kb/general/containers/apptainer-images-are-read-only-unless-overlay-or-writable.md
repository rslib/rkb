---
schema: 1
id: 2b00000029
type: fact
status: active
verified: 2026-07-04
verified_how: read
tags:
  - containers
  - apptainer
  - singularity
---

# Apptainer images are read-only unless overlay or writable-tmpfs is requested

## Statement
A `.sif` Apptainer/Singularity image is mounted read-only by default; a process inside the container that tries to write outside a bind-mounted path (for example, into `/opt` baked into the image) fails, unless the run is started with `--writable-tmpfs` or an overlay is attached.

## Evidence
`apptainer exec image.sif touch /opt/marker` failed with a read-only filesystem error; the same command with `apptainer exec --writable-tmpfs image.sif touch /opt/marker` succeeded, and the write did not persist after the container exited.
