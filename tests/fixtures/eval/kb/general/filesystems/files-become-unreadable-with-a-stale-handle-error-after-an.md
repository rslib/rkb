---
schema: 1
id: 2b00000038
type: pitfall
status: active
verified: 2026-05-03
verified_how: read
tags:
  - filesystems
  - nfs
  - stale-file-handle
---

# Files become unreadable with a stale handle error after an NFS export changes

## Symptom
A long-running process that had a file open before an NFS export was reconfigured or the server restarted now fails with:

```text
cat: file.txt: Stale file handle
```

## Cause
NFS file handles reference the file by an internal identifier on the server (not just a path). If the export is reconfigured, the underlying filesystem is remounted on the server, or the server restarts and loses track of the handle, clients holding an open reference to the old handle get `ESTALE` on the next operation.

## Fix
There is no client-side repair for an already-open stale handle; the process holding it must close and reopen the file (which usually means restarting the process). To avoid it going forward, coordinate NFS server maintenance windows with anything that keeps long-lived open file descriptors on that mount.

## Evidence
Restarting the affected process cleared the stale handle immediately, since the reopen picked up a fresh handle for the current export.
