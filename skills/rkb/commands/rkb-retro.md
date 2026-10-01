---
generated-by: rkb install (rkb install --uninstall removes this file)
description: Record the durable lessons of this session in rkb now, while the context is complete.
argument-hint: "[focus]"
---

Review the work of this session so far and record each durable lesson in rkb. Follow the rkb skill. Pass `--toon` on every rkb command.

1. List what this session taught that will matter again: a fix for an error that can come back, a rule, a decision with its reason, a recipe that took several tries. Leave out what is obvious, what is specific to this one task only, and what you cannot back up from the session.
2. For each one, run `rkb search "<words from the problem>"` first. When a lesson already covers it, extend that lesson with `rkb edit` instead of adding a close copy.
3. Otherwise write the lesson yourself: rkb runs no model, it only places, checks and commits what you give it. Take the skeleton from `rkb add --type <type> --template`, fill it, and pipe it straight in with a heredoc (`rkb add --topic <topic> <<'EOF'` ... `EOF`), with no temporary file. Keep the real error text and the command that fixed it in the lesson. Give it 3 to 4 `queries` as the skill says (a ranking aid, optional): the real error text first, then the symptom in plain words, a how-do-I question and the same problem in other words. When you extend a lesson with `rkb edit`, check its `queries` against the change.
4. Decide each lesson's scope from its claim. A claim that holds only for this project (its code, decisions, conventions or numbers) gets `when: project: <name>`. A general lesson must read correctly without this project; name the project only in Evidence. When the work taught both, write both.
5. Never invent a fix or a reason. When you are not sure a lesson is right, ask the user or leave it out.
6. At the end, say which lessons you added or extended, with their ids, or say that there were none.

Focus from the user, if any: $ARGUMENTS
