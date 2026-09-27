---
schema: 1
id: 2b00000037
type: pitfall
status: active
verified: 2026-05-02
verified_how: read
tags:
  - filesystems
  - quota
  - disk-full
---

# "No space left on device" on a shared filesystem despite a mostly empty personal directory

## Symptom
```text
$ cp bigfile.dat $SCRATCH/
cp: error writing '$SCRATCH/bigfile.dat': Disk quota exceeded
```
on a shared cluster filesystem, even though `du -sh $SCRATCH` shows only a few gigabytes used.

## Cause
The filesystem enforces a per-user block quota that is independent of how full the shared filesystem itself is; the quota can be exceeded well before the underlying disk is close to full, and du reporting a small size for the user's own directory doesn't account for quota already consumed by files elsewhere the quota system charges to the same user (shared group directories, leftover job output).

## Fix
Check the quota report directly rather than free space on the filesystem:

```sh
lfs quota -u $USER /scratch     # Lustre
quota -s                        # generic
```
and remove or move files that count against the quota, including ones in shared or group directories.

## Evidence
`lfs quota -u $USER /scratch` showed the block limit fully consumed by output left in a shared project directory the user had forgotten about; clearing it brought usage back under quota and the copy succeeded.
