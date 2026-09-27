#!/usr/bin/env bash
# Times `rkb search` (or another rkb command) on a generated knowledge base and prints p50 and p95.
# Usage: scripts/bench-search.sh [lessons] [dir] [rkb arguments...]
#   lessons  number of generated lessons (default 5000)
#   dir      where to build the knowledge base (default: a new temporary directory).
#            Point it at the NFS home on a cluster to measure what users will see.
#   rkb arguments  the command to time (default: search cmake cannot find the library header),
#            for example: scripts/bench-search.sh 5000 "" dupes
set -euo pipefail

n=${1:-5000}
dir=${2:-}
dir=${dir:-$(mktemp -d)}
shift $(($# < 2 ? $# : 2))
repo=$(cd "$(dirname "$0")/.." && pwd)

cargo build --release --quiet --manifest-path "$repo/Cargo.toml"
rkb="${CARGO_TARGET_DIR:-$repo/target}/release/rkb"

export RKB_HOME="$dir/kb" XDG_STATE_HOME="$dir/state"
"$rkb" init >/dev/null

python3 - "$RKB_HOME" "$n" <<'GEN'
import os, random, sys
root, n = sys.argv[1], int(sys.argv[2])
random.seed(7)
words = ("build link cache compile header library module install path version flag option error "
         "config target cluster node job queue memory thread rank socket file stripe write read "
         "python pip venv git rebase commit branch merge latex figure table cmake make ninja").split()
for i in range(n):
    topic = f"general/t{i % 50}"
    os.makedirs(f"{root}/{topic}", exist_ok=True)
    title = " ".join(random.choice(words) for _ in range(6)).capitalize()
    para = lambda k: " ".join(random.choice(words) for _ in range(k)) + "."
    with open(f"{root}/{topic}/l{i}.md", "w") as f:
        f.write(f"---\nschema: 1\nid: {i:010x}\ntype: pitfall\nstatus: active\nverified: 2026-09-25\n"
                f"verified_how: ran\ntags:\n  - {random.choice(words)}\n---\n\n# {title} {i}\n\n"
                f"## Symptom\n{para(30)}\n\n## Cause\n{para(40)}\n\n## Fix\n{para(40)}\n\n## Evidence\n{para(20)}\n")
GEN

python3 - "$rkb" "$n" "$@" <<'TIME'
import subprocess, sys, time
rkb, n = sys.argv[1], sys.argv[2]
cmd = sys.argv[3:] or ["search", "cmake cannot find the library header"]
subprocess.run([rkb, "search", "warm up the file cache"], stdout=subprocess.DEVNULL, check=True)
t = []
for _ in range(20):
    start = time.perf_counter()
    subprocess.run([rkb, *cmd], stdout=subprocess.DEVNULL, check=True)
    t.append((time.perf_counter() - start) * 1000)
t.sort()
p = lambda q: t[min(len(t) - 1, int(q * len(t)))]
print(f"lessons: {n}  {cmd[0]} runs: {len(t)}  p50: {p(0.50):.0f} ms  p95: {p(0.95):.0f} ms")
TIME
echo "knowledge base: $RKB_HOME"
