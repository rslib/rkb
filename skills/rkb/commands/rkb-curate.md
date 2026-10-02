---
generated-by: rkb install (rkb install --uninstall removes this file)
description: Merge near-duplicate lessons, sharpen weak ones and archive ones that do not help, in a subagent.
argument-hint: "[lesson ids or folder]"
---

Curate the rkb knowledge base in a context of its own. Pass `--toon` on every rkb command.

1. Hand the work to the curate agent, with the lesson ids or folder the user gave, if any: $ARGUMENTS.
   - In Claude Code: the Agent tool with `subagent_type: "rkb:curate"`.
   - In omp: the `task` tool with the agent `rkb-curate`.
   - In pi: the `subagent` tool with the agent `rkb-curate`, when you have that tool.
   Then report what the agent did, and any `needs_user` request it left open.
2. When no agent can run here, do its steps yourself:

__STEPS__
