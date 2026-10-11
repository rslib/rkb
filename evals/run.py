#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Evaluate the rkb skill with `claude plugin eval`, on a small model, in a throwaway home.

  uv run evals/run.py [--case NAME]... [--runs N] [--keep] [--old REV [--require-no-drop]]

Each run installs the plugin with `target/debug/rkb install claude` into a temporary HOME,
copies the chosen cases into the plugin's evals/ folder and runs them. Each case's setup.sh
builds a throwaway knowledge base. Before and after the batch the runner compares the real
knowledge base and the real state folder, and exits 1 when either changed.
With --old REV every case runs once with the skill text of that git revision and once with the
text in the working tree, and the report has one row for each text. An unknown REV exits 2.
Results go to evals/results/<timestamp>/ and are never published.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Final

ROOT: Final = Path(__file__).resolve().parent.parent
EVALS: Final = ROOT / "evals"
RKB: Final = ROOT / "target" / "debug" / "rkb"
FIXTURE: Final = ROOT / "tests" / "fixtures" / "kb"
REAL_KB: Final = Path.home() / "Personal" / "kb"
REAL_STATE: Final = Path.home() / ".local" / "state" / "rkb"
# Only the plugin under test: no memory, bundled skills, CLAUDE.md files or workflows.
CLEAN_ENV: Final = {name: "1" for name in (
    "CLAUDE_CODE_DISABLE_AUTO_MEMORY", "CLAUDE_CODE_DISABLE_ORG_MEMORY", "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS",
    "CLAUDE_CODE_DISABLE_CLAUDE_MDS", "CLAUDE_CODE_DISABLE_WORKFLOWS")}
# The plugin's tools are `mcp__rkb__*`; a plugin MCP server would be `mcp__plugin_rkb_rkb__*`.
ALLOW: Final = ["Bash", "Write", "Edit", "Read", "mcp__rkb__*", "mcp__plugin_rkb_rkb__*"]
MAX_COST_USD: Final = "2"
# The canary checks isolation, not the skill, so a default batch leaves it out.
NOT_DEFAULT: Final = {"canary"}


UUID: Final = re.compile(rb"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


class Snap:
    """The real KB and state folder: `files` maps a path to (mtime_ns, size) and `sessions` holds every
    session id already in a state file or a usage file, so later appends can be told apart."""

    def __init__(self) -> None:
        self.head = ""
        self.files: dict[Path, tuple[int, int]] = {}
        self.sessions: set[bytes] = set()
        if (REAL_KB / ".git").exists():
            self.head = self._git("rev-parse", "HEAD").strip()
            # `status --porcelain` names a changed path but not a second change to a dirty one, so stat each.
            for p in REAL_KB.glob(".rkb/usage/*.jsonl"):
                self._add(p)
            for line in self._git("status", "--porcelain", "--untracked-files=all").splitlines():
                self._add(REAL_KB / line[3:].split(" -> ")[-1].strip('"'))
        if REAL_STATE.exists():
            for p in sorted(REAL_STATE.rglob("*")):
                self._add(p)
        for p in self.files:
            if p.suffix == ".jsonl" and p.is_file():
                self.sessions |= set(UUID.findall(p.name.encode() + p.read_bytes()))

    def _git(self, *args: str) -> str:
        return subprocess.run(["git", "-C", str(REAL_KB), *args], capture_output=True, text=True).stdout

    def _add(self, p: Path) -> None:
        try:
            st = p.lstat()
        except OSError:
            return
        self.files[p] = (st.st_mtime_ns, st.st_size)


def is_live_noise(path: Path, before: Snap, size_before: int) -> bool:
    """A change that only a live harness session of the user can make: a heartbeat stamp, a folder whose
    mtime moved, or lines appended to a jsonl file that all name a session known before the batch."""
    if path.is_dir() or path.parent == REAL_STATE / "heartbeat":
        return True
    if path.suffix != ".jsonl":
        return False
    data = path.read_bytes()
    return len(data) >= size_before > 0 and set(UUID.findall(data[size_before:])) <= before.sessions


def leaks(before: Snap, after: Snap) -> tuple[list[str], int]:
    """Returns each changed path and how many changes were ignored as live-session noise."""
    changed, ignored = [], 0
    if before.head != after.head:
        changed.append(f"{REAL_KB} (HEAD {before.head[:7]} -> {after.head[:7]})")
    for path in sorted(before.files.keys() | after.files.keys()):
        old, new = before.files.get(path), after.files.get(path)
        if old == new:
            continue
        # A file that appeared or went away, or changed outside the live-noise cases, is a leak.
        if old and new and is_live_noise(path, before, old[1]):
            ignored += 1
        else:
            changed.append(str(path))
    return changed, ignored


def git_dir() -> str:
    """A folder with the real git binary. /usr/bin/git is an Xcode shim that writes a cache in the user's
    temp folder, which the agent's sandbox forbids, so every `rkb add` would fail on it."""
    found = subprocess.run(["xcrun", "--find", "git"], capture_output=True, text=True).stdout.strip()
    if found and Path(found).parent != Path("/usr/bin"):
        return str(Path(found).parent)
    for d in os.environ["PATH"].split(os.pathsep):
        if d != "/usr/bin" and (Path(d) / "git").is_file():
            return d
    return "/usr/bin"


def build_plugin(tmp: Path, cases: list[str], skill: str | None = None) -> Path:
    """Installs the plugin into tmp/home and puts the chosen cases into its evals/ folder.
    With `skill`, that text replaces the plugin's SKILL.md."""
    home = tmp / "home"
    (tmp / "bin").mkdir()
    # `rkb install claude` runs `claude plugin ...` against the temporary home; a stub keeps it to file writes.
    stub = tmp / "bin" / "claude-stub"
    stub.write_text("#!/bin/sh\necho '[]'\n")
    stub.chmod(0o755)
    env = {**os.environ, "HOME": str(home), "XDG_DATA_HOME": str(home / ".local/share"),
           "XDG_STATE_HOME": str(home / ".local/state"), "XDG_CACHE_HOME": str(home / ".cache"),
           "XDG_CONFIG_HOME": str(home / ".config"), "RKB_HOME": str(home / "Personal/kb"),
           "RKB_TEST_HOME": str(home / "Personal/kb"), "RKB_CLAUDE": str(stub)}
    home.mkdir()
    # Install asks a person at a terminal; `script` gives it a pty and the answer comes on stdin.
    ask = subprocess.Popen(["script", "-q", "/dev/null", str(RKB), "install", "claude"], env=env,
                           stdin=subprocess.PIPE, stdout=subprocess.DEVNULL)
    time.sleep(1)
    ask.communicate(b"y\n")
    if ask.returncode:
        sys.exit(f"error: rkb install claude exited {ask.returncode}")
    plugin = home / ".local/share/rkb/claude-plugin/plugins/rkb"
    if not plugin.is_dir():
        sys.exit(f"error: no plugin at {plugin}")
    if skill is not None:
        (plugin / "skills/rkb/SKILL.md").write_text(skill)
    for name in cases:
        case = EVALS / "cases" / name
        shutil.copytree(case, plugin / "evals" / name)
        shutil.copy(EVALS / "setup.sh", plugin / "evals" / name / "setup.sh")
        shutil.copytree(FIXTURE, plugin / "evals" / name / "kb", dirs_exist_ok=True)
    return plugin


def scores(doc: dict) -> dict[str, list[float]]:
    """Run scores for each case; a run with an error counts as 0."""
    return {c["name"]: [0.0 if r["error"] else r["score"] for arm in c["arms"].values() for r in (arm or [])]
            for c in doc["cases"]}


def mean(xs: list[float]) -> float:
    return sum(xs) / len(xs) if xs else 0.0


def report(docs: dict[str, dict], words: dict[str, int], old_mode: bool) -> int:
    """Prints the score of each case for each text, then one row for each text.
    Returns 1 when a run errored, the batch stopped early or, without --old, a case scored under 1."""
    per = {label: scores(doc) for label, doc in docs.items()}
    names = list(next(iter(per.values())))
    print(f"{'case':20}" + "".join(f" {label:>6}" for label in per))
    for name in names:
        print(f"{name:20}" + "".join(f" {mean(per[label].get(name, [])):6.2f}" for label in per))
    bad = 0
    for label, doc in docs.items():
        for case in doc["cases"]:
            for arm in case["arms"].values():
                for r in arm or []:
                    if r["error"]:
                        bad += 1
                        print(f"error [{label} {case['name']}]: {r['error']}")
                    elif not old_mode:
                        for g in r["graders"]:
                            if not g["passed"]:
                                print(f"failed grader [{case['name']}]: {g['name']}")
        bad += bool(doc["partial"])
    print(f"\n{'text':6} {'mean':>5} {'words':>6} {'cost':>7}")
    for label, doc in docs.items():
        flat = [x for xs in per[label].values() for x in xs]
        print(f"{label:6} {mean(flat):5.2f} {words[label]:6d} ${doc['costUsd']:6.2f}")
    low = not old_mode and any(mean(xs) < 1.0 for xs in per["new"].values())
    return 1 if bad or low else 0


def old_skill(rev: str) -> str:
    """The skill text at a git revision, or exit 2."""
    shown = subprocess.run(["git", "-C", str(ROOT), "show", f"{rev}:skills/rkb/SKILL.md"], capture_output=True, text=True)
    if shown.returncode:
        print(f"error: no skills/rkb/SKILL.md at '{rev}': {shown.stderr.strip()}", file=sys.stderr)
        sys.exit(2)
    return shown.stdout


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--case", action="append", default=[], help="case name; repeatable. Default: the task cases")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--keep", action="store_true", help="keep each run's sandbox and the temporary home")
    ap.add_argument("--old", metavar="REV", help="also run with the skill text at this git revision")
    ap.add_argument("--require-no-drop", action="store_true", help="with --old, exit 1 when new scores below old")
    args = ap.parse_args()

    known = sorted(p.parent.name for p in (EVALS / "cases").glob("*/prompt.md"))
    cases = args.case or [n for n in known if n not in NOT_DEFAULT]
    for name in cases:
        if name not in known:
            sys.exit(f"error: unknown case '{name}'; known: {', '.join(known)}")
    texts: dict[str, str | None] = {"new": None}
    if args.old:
        texts = {"old": old_skill(args.old), "new": None}
    if subprocess.run(["cargo", "build", "-q", "-p", "rkb"], cwd=ROOT).returncode:
        sys.exit("error: cargo build failed")

    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    out = EVALS / "results" / stamp
    out.mkdir(parents=True)
    tmp = Path(tempfile.mkdtemp(prefix="rkb-eval-")).resolve()
    codes: dict[str, int] = {}
    try:
        plugins = {}
        for label, text in texts.items():
            (tmp / label).mkdir()
            plugins[label] = build_plugin(tmp / label, cases, text)
        before = Snap()
        # Background QoS on macOS: efficiency cores and low I/O priority, so the laptop stays usable.
        prefix = ["taskpolicy", "-b"] if shutil.which("taskpolicy") else []
        # The agent's shell does not see the temp bin dir, so the fresh build comes first from target/debug.
        env = {**os.environ, **CLEAN_ENV, "PATH": os.pathsep.join([str(RKB.parent), git_dir(), os.environ["PATH"]])}
        for label, plugin in plugins.items():
            cmd = [*prefix, "claude", "plugin", "eval", str(plugin), "--model", "haiku", "--no-publish",
                   "--trust-plugin", "--threshold", "0", "--scaffold", "--concurrency", "3",
                   "--max-cost-usd", MAX_COST_USD, "--runs", str(args.runs), "--output-dir", str(out / label),
                   "--ablation", "none", "--allow-tools", *ALLOW]
            if args.keep:
                cmd.append("--keep-temp")
            print("+", " ".join(cmd), file=sys.stderr)
            codes[label] = subprocess.run(cmd, cwd=ROOT, env=env).returncode
        after = Snap()
    finally:
        if args.keep:
            print(f"kept: {tmp}", file=sys.stderr)
        else:
            shutil.rmtree(tmp, ignore_errors=True)

    changed, ignored = leaks(before, after)
    if ignored:
        print(f"note: ignored {ignored} change(s) made by live sessions (see evals/README.md)", file=sys.stderr)
    if changed:
        print("\nLEAK: the batch changed the real knowledge base or state folder:", file=sys.stderr)
        for k in changed:
            print(f"  {k}", file=sys.stderr)
        return 1
    docs = {}
    for label, code in codes.items():
        result = out / label / "aggregate-result.json"
        if code not in (0, 1) or not result.is_file():
            print(f"error: claude plugin eval exited {code} for {label}", file=sys.stderr)
            return 1
        docs[label] = json.loads(result.read_text())
    skill_text = {label: (text if text is not None else (ROOT / "skills/rkb/SKILL.md").read_text())
                  for label, text in texts.items()}
    status = report(docs, {k: len(v.split()) for k, v in skill_text.items()}, bool(args.old))
    print(f"results: {out}")
    if args.old and args.require_no_drop:
        flat = {label: mean([x for xs in scores(doc).values() for x in xs]) for label, doc in docs.items()}
        if flat["new"] < flat["old"]:
            print(f"new scores {flat['new']:.2f}, below old {flat['old']:.2f}", file=sys.stderr)
            return 1
    return status


if __name__ == "__main__":
    sys.exit(main())
