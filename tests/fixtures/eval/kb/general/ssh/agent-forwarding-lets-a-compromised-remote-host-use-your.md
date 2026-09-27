---
schema: 1
id: 2b00000044
type: fact
status: active
verified: 2026-04-04
verified_how: read
tags:
  - ssh
  - agent-forwarding
  - security
---

# Agent forwarding lets a compromised remote host use your local ssh key

## Statement
`ssh -A` (agent forwarding) does not copy the private key to the remote host, but it does let any process with sufficient privileges on that remote host relay signing requests through the forwarded socket, so a root-level compromise of the remote machine can use the local key to authenticate elsewhere for as long as the session is open.

## Evidence
Documentation for OpenSSH's `ForwardAgent` option explicitly warns that users with root access on the remote host can access the local agent through the forwarded socket, which is why many hardened configurations disable agent forwarding entirely in favor of `ProxyJump`.
