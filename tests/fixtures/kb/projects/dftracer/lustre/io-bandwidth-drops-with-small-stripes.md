---
schema: 1
id: 2b81d05e77
type: fact
status: active
when:
  system: tuolumne
verified: 2026-09-20
verified_how: ran
tags:
  - lustre
  - io
---

# IO bandwidth drops with small Lustre stripe counts

## Statement
Writes of large checkpoint files reach full bandwidth only with a stripe count of 8 or more.

## Evidence
The plot shows bandwidth by stripe count for a 64 GB file.

![Bandwidth by stripe count](io-bandwidth-drops-with-small-stripes.assets/bandwidth-by-stripe-count.svg)

For the build side, see [CMake and HDF5](../cmake/cmake-needs-hdf5-root.md).
