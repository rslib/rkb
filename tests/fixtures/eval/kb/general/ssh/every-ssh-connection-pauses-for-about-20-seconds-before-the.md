---
schema: 1
id: 2b00000041
type: pitfall
status: active
verified: 2026-04-01
verified_how: read
tags:
  - ssh
  - dns
  - usedns
---

# Every ssh connection pauses for about 20 seconds before the login prompt appears

## Symptom
`ssh host` reaches the remote machine and eventually logs in, but there is a consistent ~20 second delay between the TCP connection and the password or key prompt, on every single connection.

## Cause
The sshd server has `UseDNS yes` (or no explicit setting, which used to default to yes on many distributions), so it performs a reverse DNS lookup on the connecting client's IP address, plus a forward lookup to double-check the result, before proceeding. If the client's IP has no reverse DNS record, or the DNS server is slow or unreachable, sshd waits for the lookup to time out.

## Fix
Disable the reverse lookup on the server, since it is a very weak identity check:

```text
# /etc/ssh/sshd_config
UseDNS no
```
then restart sshd.

## Evidence
`tcpdump port 53` during a connection attempt showed DNS queries for the client's IP that never got a response; setting `UseDNS no` and restarting sshd removed the delay entirely.
