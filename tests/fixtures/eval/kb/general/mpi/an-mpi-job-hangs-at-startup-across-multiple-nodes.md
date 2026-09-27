---
schema: 1
id: 2b00000031
type: pitfall
status: active
verified: 2026-06-01
verified_how: read
tags:
  - mpi
  - networking
  - firewall
---

# An MPI job hangs at startup across multiple nodes

## Symptom
`mpirun -n 32 --hostfile hosts ./app` prints nothing and never progresses past rank initialization; single-node runs on any one of the hosts work fine.

## Cause
MPI runtimes open dynamic, ephemeral TCP ports between ranks for the initial handshake beyond the well-known launcher port. A host-based firewall (or a security group in a cloud cluster) that only allows a narrow port range, or blocks node-to-node traffic entirely, silently drops these connection attempts, so ranks wait forever for peers that never respond.

## Fix
Either open the port range the MPI library uses for out-of-band communication, or restrict it to a range that is already open:

```sh
# OpenMPI: restrict the dynamic port range, then allow it in the firewall
export OMPI_MCA_btl_tcp_port_min_v4=20000
export OMPI_MCA_btl_tcp_port_max_v4=20100
```

## Evidence
`tcpdump` on one node showed repeated SYN packets to another node with no SYN-ACK response; opening the restricted port range in the firewall let the handshake complete and the job proceeded normally.
