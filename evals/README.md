# rkb skill evals

These evals run the rkb plugin in Claude Code on a small model (Haiku).
Each case gives the agent a task and checks what it did.
The agent works in a throwaway home.
It never touches your real knowledge base.

```bash
uv run evals/run.py                          # the 4 task cases, 1 run each
uv run evals/run.py --old trunk --runs 3     # old skill text against the working tree
uv run evals/run.py --case canary --runs 1   # one case
uv run evals/run.py --keep                   # keep the temporary home for a look
```

`run.py` needs `cargo`, `claude` and `uv`.
It builds `target/debug/rkb`, installs the plugin with `rkb install claude` into a temporary home and runs `claude plugin eval` on it.
Results go to `evals/results/<timestamp>/`.
They are not published and not in git.
The exit code is 1 when a case fails, when the batch errors or when a leak happens.

## Old against new

`--old REV` runs every case once with `skills/rkb/SKILL.md` as it is at the git revision `REV` and once with the file in the working tree.
It installs two plugins, and the second has its `SKILL.md` replaced.
With 4 cases and `--runs 3` that is 24 case runs, 12 for each text.
An unknown `REV` exits 2 before any case runs.

The report shows the score of each case for `old` and `new`.
Then it shows one row for each text with the mean score, the size of the skill in words and the cost in dollars.
A run that errors counts as 0.
With `--old`, the exit code is 1 only when a run errors, a batch stops early or a leak happens.
Add `--require-no-drop` to also exit 1 when the mean score of `new` is below `old`.

## Cost

A canary run on Haiku costs about $0.01 to $0.05.
A task case costs under $0.01 a run.
Measured with `--old HEAD --runs 1`, the whole batch (8 runs) cost about $0.02.
So 24 runs cost about $0.06.
Each `claude plugin eval` call stops at $2 (`--max-cost-usd 2`).
Run only the cases you need while you work.

## Isolation

Each case has a `setup.sh`.
The runner copies it, with the fixture knowledge base from `tests/fixtures/kb`, into the case.
`claude plugin eval --scaffold` runs it with `HOME` and the `XDG_*` folders in a temporary directory.
The agent's shell uses that same `HOME`.
Your fish profile is not loaded there, so `RKB_HOME` from `~/.config/fish/conf.d/env.fish` does not reach the agent.

`setup.sh` does four things:

- It refuses to run when `HOME` is your real home.
- It copies the fixture to `$HOME/Personal/kb`, which is the default knowledge base of rkb, and runs `git init` there.
- It writes `$HOME/.zshenv`, which pins `RKB_HOME`, `RKB_TEST_HOME` and the four `XDG_*` folders to the temporary home.
- It puts a `git` wrapper first on `PATH`.
  `/usr/bin/git` is an Xcode shim that writes in the real temp folder, and the sandbox forbids that, so `rkb add` would fail.

The runner puts `target/debug` first on the agent's `PATH`, so the agent runs the fresh `rkb`.
The agent's shell does not see the temporary bin folder of the runner, so it cannot be used for this.

## Leak guard

Your live sessions use `~/Personal/kb` and `~/.local/state/rkb` while a batch runs.
So the runner compares both before and after the batch.
If they differ, it names each changed path and exits 1.

It compares:

- `git rev-parse HEAD` of `~/Personal/kb`.
- The `mtime` and size of each file in `git status --porcelain` of `~/Personal/kb` and of `.rkb/usage/*.jsonl`.
  `git status` alone does not show a second change to a file that is already dirty.
- The `mtime` and size of each path under `~/.local/state/rkb`.

It ignores three kinds of change, because a live session makes them and an eval run cannot:

- A new `mtime` on a folder.
- A new `mtime` on `heartbeat/*`.
- Lines added to a `.jsonl` file when all session ids in the new lines were already in a state or usage file before the batch.
  An eval run is a new session.
  If it wrote to the real state, it would add a new session id, a new `sessions/<id>.jsonl` file or both, and the guard would catch that.

The runner prints how many changes it ignored.
A new session that you start during a batch also fails the guard.
Run the batch again.

## Cases

Each case is a folder in `evals/cases/` with `prompt.md`, `case.yaml` and `graders/*.md`.
A `work/` folder in a case is copied into the working folder of the agent.
A `kb/` folder in a case adds lessons to the fixture knowledge base for that case only.
A run is noisy, so use `--runs 3` or more before you trust a difference.

The default set is the four task cases.
Their prompts do not mention rkb.
The skill must make the agent act.

- `search-first` reports a link error that a fixture lesson covers.
  It must run `rkb search` (weight 3).
- `remember` says "remember: ..." with a durable fact.
  It must run `rkb add` or `rkb note` (weight 3).
- `report-worked` has an `export.py` that fails with error E17 until `EXPORT_PROFILE` has a value that only a lesson names.
  The agent must run `rkb used <id> --worked` (weight 3).
- `no-write` renames a variable.
  It must not run `rkb add` or `rkb note` (weight 3, max 0).
- `canary` is not in the default set.
  Run it with `--case canary`.
  It runs `rkb doctor` and adds a lesson.
  It fails when the doctor output names `/Users/rayandrew/Personal/kb`.
  Run it first after you change the isolation.
