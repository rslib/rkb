"""A/B test of rkb on real agent tasks: the same tasks with rkb's hooks and tools on, and off.

Each task in tasks/<name>/ has prompt.txt, its files, an optional setup.sh and a check.sh that exits 0
when the task is done. Tasks named c-* are controls that no lesson covers. Every run gets a fresh copy
of the task and, with rkb on, a fresh clone of the knowledge base and a fresh rkb state folder, so runs
never see each other. Claude Code runs headless with no user settings, plugins or MCP servers; the
`on` arm adds only rkb's hooks (from the installed plugin) and the rkb MCP server.

  python3 scripts/agent-ab/run.py [--reps 2] [--model sonnet] [--jobs 6] [--only trigraph,c-fizz]
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
KB = os.environ.get("RKB_HOME", os.path.expanduser("~/Personal/kb"))
PLUGIN_HOOKS = os.path.expanduser("~/.local/share/rkb/claude-plugin/plugins/rkb/hooks/hooks.json")


def claude(prompt, cwd, model, env, extra):
    cmd = ["claude", "-p", "--model", model, "--setting-sources", "", "--strict-mcp-config", "--permission-mode", "bypassPermissions",
           "--max-turns", "30", "--max-budget-usd", "1", "--no-session-persistence", "--output-format", "stream-json", "--verbose"] + extra
    start = time.time()
    p = subprocess.run(cmd, input=prompt, cwd=cwd, env=env, capture_output=True, text=True, timeout=900)
    tools, result = {}, {}
    for line in p.stdout.splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        if e.get("type") == "assistant":
            for b in e["message"].get("content", []):
                if b.get("type") == "tool_use":
                    tools[b["name"]] = tools.get(b["name"], 0) + 1
        if e.get("type") == "result":
            result = e
    return result, tools, time.time() - start


def run_one(task, arm, rep, model):
    src = os.path.join(HERE, "tasks", task)
    with tempfile.TemporaryDirectory() as tmp:
        work = os.path.join(tmp, "work")
        shutil.copytree(src, work)
        os.remove(os.path.join(work, "check.sh"))
        prompt = open(os.path.join(work, "prompt.txt")).read()
        os.remove(os.path.join(work, "prompt.txt"))
        if os.path.exists(os.path.join(work, "setup.sh")):
            subprocess.run(["sh", "setup.sh"], cwd=work, capture_output=True)
            os.remove(os.path.join(work, "setup.sh"))
        env = dict(os.environ)
        for k in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "RKB_SESSION", "RKB_OBSERVER"]:
            env.pop(k, None)
        extra = []
        state = os.path.join(tmp, "state")
        if arm == "on":
            kb = os.path.join(tmp, "kb")
            subprocess.run(["git", "clone", "-q", KB, kb], check=True)
            env.update(RKB_HOME=kb, XDG_STATE_HOME=state)
            hooks = json.load(open(PLUGIN_HOOKS))
            settings = os.path.join(tmp, "settings.json")
            json.dump(hooks, open(settings, "w"))
            mcp = os.path.join(tmp, "mcp.json")
            json.dump({"mcpServers": {"rkb": {"command": "rkb", "args": ["mcp"], "env": {"RKB_HOME": kb, "XDG_STATE_HOME": state}}}}, open(mcp, "w"))
            extra = ["--settings", settings, "--mcp-config", mcp]
        result, tools, secs = claude(prompt, work, model, env, extra)
        shutil.copy(os.path.join(src, "check.sh"), os.path.join(work, "check.sh"))
        ok = subprocess.run(["sh", "check.sh"], cwd=work, capture_output=True, timeout=300).returncode == 0
        injected = 0
        sessions = os.path.join(state, "rkb", "sessions")
        if os.path.isdir(sessions):
            for f in os.listdir(sessions):
                injected += sum('"injected"' in l for l in open(os.path.join(sessions, f)))
        usage = result.get("usage", {})
        return {
            "task": task, "arm": arm, "rep": rep, "pass": ok,
            "turns": result.get("num_turns"), "cost": result.get("total_cost_usd"), "secs": round(secs, 1),
            "tokens_in": (usage.get("input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0) + (usage.get("cache_creation_input_tokens") or 0),
            "tokens_out": usage.get("output_tokens"),
            "rkb_tools": {k: v for k, v in tools.items() if "rkb" in k}, "injected": injected,
            "hooks_ran": os.path.isdir(os.path.join(state, "rkb")), "model": model,
            "error": result.get("subtype") if result.get("is_error") else None,
        }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reps", type=int, default=2)
    ap.add_argument("--model", default="sonnet")
    ap.add_argument("--jobs", type=int, default=6)
    ap.add_argument("--only", default="")
    ap.add_argument("--out", default=os.path.join(HERE, "results.jsonl"))
    a = ap.parse_args()
    tasks = sorted(os.listdir(os.path.join(HERE, "tasks")))
    if a.only:
        tasks = [t for t in tasks if t in a.only.split(",")]
    jobs = [(t, arm, r) for r in range(a.reps) for t in tasks for arm in ("off", "on")]
    rows = []
    with ThreadPoolExecutor(a.jobs) as ex, open(a.out, "a") as out:
        for row in ex.map(lambda j: run_one(*j, a.model), jobs):
            rows.append(row)
            out.write(json.dumps(row) + "\n")
            out.flush()
            print(json.dumps(row), flush=True)
    summarize(rows)


def summarize(rows):
    def agg(rs):
        n = len(rs)
        f = lambda k: sum(r[k] or 0 for r in rs) / max(1, n)
        return f"pass {sum(r['pass'] for r in rs)}/{n}, turns {f('turns'):.1f}, cost ${f('cost'):.3f}, {f('secs'):.0f}s, in {f('tokens_in') / 1000:.0f}k tok"
    for group, pred in (("lesson tasks", lambda r: not r["task"].startswith("c-")), ("controls", lambda r: r["task"].startswith("c-"))):
        for arm in ("off", "on"):
            print(f"{group:13} {arm:3}: {agg([r for r in rows if pred(r) and r['arm'] == arm])}")
    print("per task (off -> on pass):")
    for t in sorted({r["task"] for r in rows}):
        p = lambda arm: sum(r["pass"] for r in rows if r["task"] == t and r["arm"] == arm)
        used = sum(1 for r in rows if r["task"] == t and r["arm"] == "on" and (r["rkb_tools"] or r["injected"]))
        print(f"  {t:10} {p('off')} -> {p('on')}   runs where rkb was used: {used}")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--summary":
        summarize([json.loads(l) for l in open(sys.argv[2])])
    else:
        main()
