---
schema: 1
id: 2b00000026
type: pitfall
status: active
verified: 2026-07-01
verified_how: read
tags:
  - containers
  - selinux
  - bind-mount
---

# A container cannot write to a bind-mounted directory on an SELinux host

## Symptom
```text
$ podman run -v /data:/data myimage touch /data/out.txt
touch: cannot touch '/data/out.txt': Permission denied
```
even though the host directory is world-writable.

## Cause
On an SELinux-enforcing host, bind-mounted directories keep their original SELinux context, which usually does not include the label containers are allowed to access. Standard Unix permission bits are satisfied, but SELinux still denies the access.

## Fix
Relabel the mount for container access using the `:z` (shared) or `:Z` (private) mount flag:

```sh
podman run -v /data:/data:z myimage touch /data/out.txt
```

## Evidence
Adding `:z` to the volume flag let the container write to `/data`; `ausearch -m avc -ts recent` had shown denials referencing the mount before the flag was added.
