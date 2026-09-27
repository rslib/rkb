---
schema: 1
id: 2b00000043
type: recipe
status: active
verified: 2026-04-03
verified_how: ran
tags:
  - ssh
  - proxyjump
  - config
---

# Reach an internal host through a jump host without typing two ssh commands

## When to use
When an internal machine is only reachable from a bastion or jump host, and connecting normally means `ssh bastion` followed by a second `ssh internal-host` from there.

## Steps
Add both hosts to `~/.ssh/config` with `ProxyJump`:

```text
Host bastion
    HostName bastion.example.org
    User alice

Host internal-host
    HostName 10.0.4.12
    User alice
    ProxyJump bastion
```
Then `ssh internal-host` connects through the bastion transparently, and tools like `scp` and `rsync` that shell out to ssh work the same way.

## Evidence
`ssh internal-host` opened a single interactive session through the bastion with no manual second hop, and `scp file internal-host:/tmp/` copied through the same path without extra flags.
