---
schema: 1
id: 2b00000042
type: pitfall
status: active
verified: 2026-04-02
verified_how: read
tags:
  - ssh
  - mtu
  - networking
---

# An ssh session freezes partway through a large file transfer or scrollback

## Symptom
An interactive ssh session connects fine and small commands work, but running `cat largefile.txt` or `scp` of a big file stalls completely partway through and never resumes, without an explicit error.

## Cause
A path MTU black hole: some router or VPN link along the route drops packets larger than its MTU instead of sending back the ICMP "fragmentation needed" message that would let the sender adjust. Small packets (an interactive keystroke) get through fine; once ssh's TCP window fills with larger packets carrying bulk data, those packets vanish silently and the connection hangs waiting for an acknowledgment that never comes.

## Fix
Lower the MTU on the client side, or force TCP to clamp its maximum segment size, to avoid packets ever needing fragmentation on the problem link:

```sh
ssh -o 'IPQoS=throughput' -o 'Ciphers=aes128-ctr' host   # rules out cipher overhead first
ip link set dev tun0 mtu 1400                             # if over a VPN
```

## Evidence
Lowering the VPN interface's MTU from 1500 to 1400 made the same large file transfer complete without stalling, confirming a path MTU issue rather than an ssh configuration problem.
