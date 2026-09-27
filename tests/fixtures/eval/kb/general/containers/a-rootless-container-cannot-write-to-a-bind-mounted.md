---
schema: 1
id: 2b00000027
type: pitfall
status: active
verified: 2026-07-02
verified_how: read
tags:
  - containers
  - podman
  - rootless
  - uid-mapping
---

# A rootless container cannot write to a bind-mounted directory owned by the invoking user

## Symptom
```text
$ podman run --rm -v $PWD/out:/out myimage sh -c 'echo hi > /out/file'
sh: /out/file: Permission denied
```
even though `$PWD/out` is owned by the current user on the host, and SELinux is disabled.

## Cause
Rootless podman maps the container's internal root (and other UIDs) to a range of unprivileged host UIDs via `/etc/subuid`. The process inside the container that writes the file is not UID 0 from the host's point of view; it's some UID in the mapped range that does not match the host directory's owner.

## Fix
Either run the container process as the mapped UID that owns the directory, or use podman's automatic ownership flag:

```sh
podman run --rm --userns=keep-id -v "$PWD/out:/out" myimage sh -c 'echo hi > /out/file'
```

## Evidence
`--userns=keep-id` mapped the container's user to the same UID as the host user, and the write succeeded without changing host directory permissions.
