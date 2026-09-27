---
schema: 1
id: 2b00000039
type: fact
status: active
verified: 2026-05-04
verified_how: read
tags:
  - filesystems
  - lustre
  - striping
---

# Lustre striping trades single-file bandwidth against small-file metadata overhead

## Statement
Spreading a file across more Lustre OSTs (a higher stripe count) increases aggregate bandwidth for large sequential reads and writes of that single file, but it also means every open of that file touches more OSTs, which adds overhead for workloads dominated by many small files rather than a few large ones.

## Evidence
Setting `lfs setstripe -c 8` on a directory of large checkpoint files roughly matched the expected multiple of single-OST bandwidth on a striped write benchmark, while applying the same stripe count to a directory holding hundreds of thousands of small log files increased average file-open latency measurably.
