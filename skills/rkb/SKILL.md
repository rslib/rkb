---
name: rkb
description: Read and check a personal knowledge base of lessons learned (pitfalls, recipes, facts, decisions, preferences) with the rkb CLI. Use when you need to look at a lesson, check the knowledge base after a change, or write a lesson file in the right folder and format.
---

# rkb

rkb manages a git repository of Markdown lessons at `$RKB_HOME` (default `~/Personal/kb`). Output is TOON when piped; read it as it is. Do not add `--format human` or `--format json` yourself: TOON is the format meant for you, and you tell the user the result in your own words. Only a script that parses the output uses `--format json`.

- Run `rkb` (or `rkb context` for 3 short lines) at the start of a task. It shows the project and system rkb matched here, lesson counts, pending requests and uncommitted changes.
- Run `rkb doctor` when something looks wrong. Each check that is not `ok` has a `fix` line; run it or show it to the user.
- Setup, once per machine: `rkb init` creates a new knowledge base, or `rkb init --clone <url>` sets up an existing one on a second machine. Then the user runs `rkb install` in a terminal to put this skill, the hooks and the confirm gate into Claude Code, pi and omp (in pi and omp the hooks are an extension file). If you run `rkb install` yourself, it returns `needs_user`; show the question to the user.
- Run `rkb list` for a table of contents: every scope, project, system and topic with lesson counts. Run `rkb list <scope>` for one scope, such as `rkb list projects/dftracer`.
- Run `rkb list <topic folder>`, such as `rkb list general/cpp`, to see its lessons grouped by their first tag, with id, type, status, tags and conditions. Archived and superseded lessons come last.
- Run `rkb search "<words from the problem or the error>"` first when you hit a problem. Results show `applies` for this place; act only on `yes`, and check a `unknown` lesson (run its `Check`, or ask the user) before you rely on it. A `hidden:` line counts lessons that do not apply here; `--all` shows them and other projects' lessons.
- Run `rkb search --literal "<exact error text>"` for an exact string, or `--regex` for a pattern.
- Run `rkb find <rough name>` when you know roughly what a lesson is called.
- When a search missed a lesson that exists, add the query and the lesson id to `$RKB_HOME/eval/queries.toml` as a `[[query]]` table (`text`, `expect`), so `rkb eval` keeps testing it.
- Run `rkb show <id>` to read one lesson in full. The id is the `id:` field in the lesson's frontmatter.
- Run `rkb lint` after you change any file in the knowledge base. Fix every error before you commit. Add `--full` if a message is cut.
- Run `rkb lint --fix` to rewrite flow-style frontmatter in block style. It does not commit.
- Every git commit in the knowledge base runs `rkb lint --staged`. Do not skip the hook.
- Run `rkb sync` to share lessons with the user's other machines (`--remote <name>` for a cluster clone, `--bundle <file>` for a machine with no ssh path; the user carries the file). It pulls, checks, and pushes only when lint has no errors. On a `conflict` error, show the files and the `fix` commands to the user; never resolve a conflict yourself. On `status: not_pushed`, fix each error with `rkb edit`, then run `rkb sync` again.
- Run `rkb <command> --help` for flags and an example.

## Writing lessons

Never write or change a lesson file with your own tools. rkb checks it, places it and commits it.

- Run `rkb add --type pitfall --template` to get a skeleton. Fill it and pipe it: `rkb add --topic <topic> < lesson.md`. Do not write `id`, `schema`, `status` or the file path; rkb sets them. rkb picks the scope folder from `when`.
- Run `rkb show <id>` before you change a lesson. Pipe the whole new file with the hash it printed: `rkb edit <id> --base <hash> < lesson.md`. A `conflict` error means the lesson changed; read it again and merge.
- Run `rkb flag <id> --reason "<what failed>"` when a lesson looks wrong and you cannot fix it now. Fix it or flag it; never leave it silently wrong.
- Run `rkb supersede <id> --by <new id> --reason "<why>"` when a lesson is wrong and another lesson now says the right thing. Run `rkb archive <id or folder> --reason "<why>"` when a lesson is still true but no longer matters, such as for a retired system. Both hide lessons from search, so they return `needs_user`; show the question to the user. `rkb unarchive <id>` brings a lesson back without a question.
- Run `rkb review` when the user asks to clean up the knowledge base. It lists lessons that may no longer matter, with the reasons, and changes nothing. Show the list; archive only the lessons the user agrees to.
- Run `rkb used <id> --worked` after you apply a lesson and the task succeeds. Run `rkb used <id> --failed --reason "<why>"` when it did not work; this also flags it.
- Run `rkb log <id>` for the history of a lesson.
- Run `rkb move <id> <topic folder>` to put a lesson in another topic, and `rkb rename <id> <slug>` to change its file name. Both rewrite every link to the lesson. Never move or rename lesson files with your own tools; the links would break.
- To bring in existing notes (a `lessons-learned.md`, a notes folder), split them into one lesson per file in a scratch folder outside the knowledge base: `<dir>/<topic>/<name>.md`, each written from `rkb add --type <t> --template`. Keep the user's words and evidence; do not shorten. Then run `rkb import <dir>`. If it lists invalid files, fix them and run it again. Otherwise it returns one `needs_user` report: show it to the user and wait for their choice.

## When rkb needs the user

Some decisions belong to the user: a topic name that looks like a typo of an existing topic, a looser label, and actions such as installing or breaking a lock. Then rkb writes nothing, exits with code 3 and prints `status: needs_user` with a `question`, `options` and a `next` command.

1. Show the question and the options to the user. Do not choose for them.
2. Wait for their answer.
3. Run `rkb confirm <request> --choice "<option they chose>"`. When it returns `needs_terminal`, tell the user to run the command in a separate terminal window (the `fix` line has it). You cannot answer the terminal prompt yourself, and Claude Code's `!` prefix has no terminal either, so do not suggest `! rkb confirm`.

A request expires after one hour. Run the original command again after that.

A write can also succeed with `notes`. Read them and tell the user:
- `created the folder ...`: rkb made a new topic, project or system folder. Say so, in case the name is wrong.
- `looks like <id> ...`: a similar lesson exists in the same topic. Offer to merge the two with `rkb show` and `rkb edit`; do not merge without the user.

For a lesson in a project that has no folder yet, run `rkb add` from inside that project's repository: when the repository's remote ends in the project name, rkb records the remote in `projects/<p>/README.md` so it recognizes the repository from then on.

Pass `--project <name>` or `--system <name>` when rkb matched the wrong place or none.

## Notes from rkb hooks

In Claude Code, pi and omp, rkb hooks can add short lines that start with `rkb:`.

- At session start: the project and system rkb matched and the lesson counts.
- After a failed command: a lesson that may explain the failure. It is a guess from the error text. Read it with `rkb show <id>` and check that it applies before you act on it.
- At the end of a turn, only when the user turned it on: a note that the session fixed a failure or got a correction. Record a lesson with `rkb add` only when something durable was learned. Otherwise ignore the note.
- Every `rkb confirm` shows the user a permission prompt (a dialog in pi and omp) with the question and your choice. This is intended; do not try to avoid it.
- In pi or omp print or json mode there is no dialog, so `rkb confirm` is blocked. Ask the user to run the command in their own terminal.

## Where a lesson goes

```text
general/<topic>/<file>.md               true anywhere
projects/<project>/<topic>/<file>.md    true for one project
systems/<system>/<topic>/<file>.md      true on one machine or site
```

- The scope folder follows `when`. One `when.project` means `projects/<project>/`. Otherwise one `when.system` means `systems/<system>/`. Several projects or systems stay in `general/`.
- A topic is the thing you work with: `cpp`, `cmake`, `lustre`, `flux`, `latex`, `git`, `writing`, `peer-review`. A quality such as `performance` or `debugging` is a tag, not a topic.
- Use an existing topic folder when one fits. List them with `ls $RKB_HOME/general`. Ask the user before you create a new topic.
- Never nest topics. `general/cpp/regex/` is an error. Use `general/cpp/` and the tag `regex`.
- Put the most specific tag first. `rkb list` groups lessons by their first tag.
- One lesson per file. The file name is a readable slug of the title. Folder names are lowercase letters, digits and hyphens.

## Folder notes

A folder can have a `README.md`. The user writes its body. Its frontmatter holds the facts rkb needs:

```markdown
---
aliases:
  - c++
  - cxx
---

# C and C++
```

- Topic note: `aliases` (other names for the topic; never create a folder with an alias name), `labels`.
- `projects/<project>/README.md`: `remotes`, `root_commit`, `retired`, `labels`.
- `systems/<system>/README.md`: `hostname`, `match_env`, `facts`, `retired`, `labels`.
- Every note can also have `meta` for the user's own data. Never change the body of a note.

## Lesson format

```markdown
---
schema: 1
id: 7f3a9c2b41
type: pitfall
status: active
when:
  system: tuolumne
  hdf5: "1.12:1.14.2"
verified: 2026-09-25
verified_how: ran
tags:
  - cmake
  - hdf5
labels:
  sensitivity: internal
---

# CMake cannot find HDF5 unless HDF5_ROOT is set

## Symptom
...
```

- `id`: 10 lowercase hex characters, unique in the knowledge base.
- `type`: `pitfall`, `recipe`, `fact`, `decision` or `preference`.
- `status`: `active`, `stale` (needs `stale_reason`), `superseded` (needs `superseded_by`) or `archived`.
- `verified_how`: `ran` (you ran the fix), `read` (from documentation) or `told` (the user said it). Never write `checked`. Only `rkb verify` sets it, and the pre-commit hook rejects it.
- `when`, `labels` and `meta` are maps. Any other top-level key is an error.
- Write frontmatter in block style: one key per line and one list item per line, indented under its key. Never write `[a, b]` or `{k: v}`.
- A lesson without `labels` takes the `labels` of the nearest folder note, and is `internal` when no note sets one.

## Conditions: `when`

`when` says where a lesson holds. Every key must match. rkb reports `applies: yes`, `no` or `unknown`, and `rkb show <id>` gives the reason per key.

```yaml
when:
  hdf5: "1.12:1.14.2"     # version range; either side may be empty: "1.12:", ":1.14"
  compiler: "gcc@12:"     # name@range; "gcc@12" alone means any 12.x
  gpu:                    # any of these
    - a100
    - mi300a
  os: linux               # plain value: exact text
  path: "src/io/**"       # glob on the file you work on
  since: 1a2b3c4          # the fix needs this commit in HEAD; `until` is the opposite
```

- Always quote versions. YAML turns `1.10` into the number `1.1`; lint rejects it.
- A plain value is exact text: `hdf5: "1.14"` does not match `1.14.3`. Write `"1.14:1.14"` for any 1.14.x.
- Facts come from `--with key=value`, then built-ins (`system`, `project`, `os`, `branch`, `path` only via `--with path=<file>`, `since`, `until`), then the `facts` in the system's folder note. A key with no fact gives `unknown`, never `yes`.
- Test a lesson against another place with `rkb show <id> --with hdf5=1.14.3 --system tuolumne`.

Required H2 headings, in this order:

| type | headings |
| --- | --- |
| pitfall | Symptom, Cause, Fix, Evidence |
| recipe | When to use, Steps, Evidence |
| fact | Statement, Evidence |
| decision | Context, Decision, Why, Rejected options |
| preference | Rule, Why, How to apply |

- Start the body with one `# Title` line.
- A superseded lesson also needs `## Why superseded`. An archived lesson needs `## Why archived`.
- No required section can be empty.
- `## Environment`, `## Check` and `## Probe` are optional. `Check` and `Probe` hold exactly one fenced `sh` or `bash` block.
- Use fenced code blocks with a language tag. Indented code blocks are errors.
- Link other lessons with relative Markdown links, such as `[HDF5 static libs](../../general/hdf5/static-libs-need-flag.md)`. Lint checks inline links only, not reference-style links.
- Images go in `<lesson-slug>.assets/` next to the lesson: `png`, `jpg`, `webp` or `svg`, at most 1 MB, and linked from the lesson.

## Keep lessons correct

- Keep every fact needed to trust and reproduce the lesson. Never shorten a lesson to save space.
- Put only the key lines of a log in `Evidence`. Never paste a raw log.
- Never write personal or site data: usernames, email addresses, absolute home or scratch paths, job IDs. Use `$HOME`, `$USER`, `$PROJECT_ROOT` or `$SCRATCH`. Lint fails on a match. If a match is a false alarm, ask the user to add the exact text to `leak.allow` in `kb.toml`.

## Writing style

Write in ASD-STE100 Simplified Technical English:

- Short sentences. One instruction per sentence.
- Active voice. Simple words.
- Use the imperative for instructions: "Set HDF5_ROOT", not "HDF5_ROOT should be set".
