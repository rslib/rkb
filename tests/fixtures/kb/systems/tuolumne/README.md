---
hostname:
  - "tuolumne*"
  - "tuo[0-9]*"
match_env:
  LCSCHEDCLUSTER: tuolumne
facts:
  gpu: mi300a
labels:
  sensitivity: public
---

# Tuolumne

LLNL system with AMD MI300A nodes and Lustre scratch.
