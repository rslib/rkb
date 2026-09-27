---
schema: 1
id: 2b00000030
type: decision
status: active
verified: 2026-07-05
verified_how: told
tags:
  - containers
  - podman
  - docker
  - hpc
---

# Use rootless Podman instead of Docker for shared HPC login nodes

## Context
Researchers needed to run containerized workloads on a shared login node where nobody has root access and a persistent background daemon running as root is not something the cluster operators would allow.

## Decision
Standardize on rootless Podman for interactive container use on shared nodes, keeping Docker only for local development machines where a user already has admin rights.

## Why
Podman runs containers as the invoking user with no privileged daemon, matching the security model of a shared multi-user machine; Docker's daemon model requires either root or membership in a group that is effectively root-equivalent.

## Rejected options
Requesting Docker daemon access with a restricted group: rejected by cluster operators as equivalent to granting root. Running Docker inside a personal VM per user: adds a virtualization layer and resource overhead the shared node doesn't have room for.
