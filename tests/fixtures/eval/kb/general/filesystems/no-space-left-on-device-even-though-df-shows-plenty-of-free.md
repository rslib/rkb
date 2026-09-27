---
schema: 1
id: 2b00000036
type: pitfall
status: active
verified: 2026-05-01
verified_how: read
tags:
  - filesystems
  - inodes
  - disk-full
---

# "No space left on device" even though df shows plenty of free space

## Symptom
```text
$ touch newfile
touch: cannot touch 'newfile': No space left on device
$ df -h .
Filesystem      Size  Used Avail Use% Mounted on
/dev/sda1       200G   80G  110G  43% /
```

## Cause
The filesystem ran out of inodes, not disk blocks. Each file, however small, consumes one inode; a directory tree with millions of tiny files (a build cache, a mail spool, a job scratch directory full of small output files) can exhaust the fixed inode table long before the space it occupies fills the disk.

## Fix
Check inode usage separately from block usage, then remove or archive small files in bulk:

```sh
df -ih .
find . -type f | wc -l
```

## Evidence
`df -ih .` showed `100% IUse`, while `df -h .` showed over half the disk free; deleting a directory of several million cached temporary files freed inodes and let `touch` succeed again.
