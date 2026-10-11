---
tags: [canary]
max_turns: 14
timeout_seconds: 240
allowed_tools: [Read, Glob, Grep, Skill, Bash, Write, Edit]
---

Run `rkb doctor`. Save its full output in a file named DOCTOR.txt in the current folder. Then tell me which knowledge base rkb uses.

Then add one lesson to that knowledge base, in the existing topic general/cpp: when a CMake build cannot find HDF5, set HDF5_ROOT. Keep it to one line. Use rkb to add it.
