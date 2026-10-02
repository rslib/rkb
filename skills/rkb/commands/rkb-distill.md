---
generated-by: rkb install (rkb install --uninstall removes this file)
description: Turn triaged rkb inbox candidates (notes, session extracts and observed notes) into lessons, in a subagent.
argument-hint: "[item ids]"
---

Distill the rkb inbox into lessons in a context of its own. Pass `--toon` on every rkb command.

1. Run `rkb inbox triage`, which decides cheaply which inbox candidates are worth a lesson. In pi or omp, run `rkb inbox triage --ask pi` (or `--ask omp`) instead, so a small model also decides the unsure ones.
2. Hand the rest to the distill agent, with the item ids the user gave, if any: $ARGUMENTS.
   - In Claude Code: the Agent tool with `subagent_type: "rkb:distill"`.
   - In omp: the `task` tool with the agent `rkb-distill`.
   - In pi: the `subagent` tool with the agent `rkb-distill`, when you have that tool.
   Then report what the agent did, and any `needs_user` request it left open.
3. When no agent can run here, do its steps yourself:

__STEPS__
