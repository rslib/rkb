---
schema: 1
id: 2b00000045
type: recipe
status: active
verified: 2026-04-05
verified_how: ran
tags:
  - ssh
  - controlmaster
  - performance
---

# Speed up repeated ssh connections to the same host with connection multiplexing

## When to use
When a script or workflow opens many short-lived ssh connections to the same host in a row (running several remote commands, or many small `scp` transfers), and each new connection pays the full TCP and authentication handshake cost.

## Steps
Add multiplexing settings to `~/.ssh/config`:

```text
Host *
    ControlMaster auto
    ControlPath ~/.ssh/sockets/%r@%h-%p
    ControlPersist 10m
```
The first connection opens a real session and leaves a control socket open; subsequent connections to the same host reuse it instead of renegotiating.

## Evidence
A script running twenty short remote commands in sequence dropped from several seconds of accumulated handshake overhead to near-instant after the first connection, confirmed by watching `ssh -v` skip key exchange on later invocations.
