Curate the rkb knowledge base: keep it small, correct and useful. Follow the rkb skill. Pass `--toon` on every rkb command. You cannot ask the user: when rkb returns `needs_user`, leave the request open, never run `rkb confirm`, and name the request in your report.

1. Collect up to 5 candidates, or take the ids or folder your task names. Sources: `rkb dupes --check` first, then `rkb dupes` (pairs that may say the same thing; Jev hides pairs it rates different and adds a verdict per pair: `same` or `extends` means merge, `conflicts` means supersede the wrong one or ask the user, no verdict means read both), `rkb review` (lessons that failed repeatedly, never helped, are often irrelevant, unused, stale or out of date) and the `quality/...` warnings of `rkb lint`.
2. Read each candidate with `rkb show <id>`, and both lessons of a duplicate pair. Similar words do not always mean the same lesson; check the claims and the `when` conditions.
3. For each candidate, do one of these, one rkb write per action, with a concrete reason:
   - Same lesson twice: `rkb edit` the one to keep so it holds every fact and every piece of evidence from both and the union of their `queries` (no duplicates, at most 8), then `rkb supersede <other> --by <kept> --reason "merged into <kept>"`. Never drop a fact while merging.
   - Weak lesson (vague title, no real evidence, a fix too short to follow): `rkb edit` it with what you know for sure (and give it `queries` when it has none), or `rkb flag <id> --reason "<what is missing>"` when you cannot fix it.
   - Often irrelevant (`often_irrelevant`: it keeps being offered for problems it does not fit): `rkb edit` it with a sharper title, tags or `when` conditions so search offers it only for its own problem; remove a query that is too generic. Do not archive it for this reason.
   - Wrong lesson: `rkb supersede` it by the right one, or `rkb flag` it.
   - True but no longer useful (a retired system, never helped, unused for months): `rkb archive <id> --reason "<why>"`.
   - Fine as it is: leave it and say why.
4. Never invent facts or evidence. Keep the user's words and the real error text.
5. Report per candidate what you did, with the ids, so the user can check it with `rkb changes`, and any `needs_user` request you left open.
