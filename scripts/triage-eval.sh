#!/usr/bin/env bash
# Runs `rkb inbox triage` on the synthetic notes in tests/fixtures/triage/notes.tsv against a copy of the
# synthetic search knowledge base, with the rerank chain given (default: jev, bm25), and prints how often
# the verdict matches the expected one. Uses this machine's config.toml for the Jev key.
# Usage: scripts/triage-eval.sh [chain]   for example: scripts/triage-eval.sh '"bm25"'
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
chain=${1:-'"jev", "bm25"'}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

cargo build --quiet --manifest-path "$repo/Cargo.toml"
rkb="${CARGO_TARGET_DIR:-$repo/target}/debug/rkb"
cp -R "$repo/tests/fixtures/search/kb" "$dir/kb"
sed -i.bak "s/^chain = .*/chain = [$chain]/" "$dir/kb/kb.toml" && rm "$dir/kb/kb.toml.bak"
git -C "$dir/kb" init -q && git -C "$dir/kb" add -A && git -C "$dir/kb" -c user.name=T -c user.email=t@example.org commit -q -m fixture

mkdir -p "$dir/state/rkb/inbox"
{
  printf -- '---\nkind: observed\ntime: %s\n---\n\n' "$(date +%s)"
  cut -f2 "$repo/tests/fixtures/triage/notes.tsv"
} >"$dir/state/rkb/inbox/0000000001.md"

RKB_HOME="$dir/kb" XDG_STATE_HOME="$dir/state" "$rkb" inbox triage --format json >/dev/null
RKB_HOME="$dir/kb" XDG_STATE_HOME="$dir/state" "$rkb" inbox show 0000000001 --format json >"$dir/show.json"
python3 - "$repo/tests/fixtures/triage/notes.tsv" "$dir/show.json" ${VERBOSE:+-v} <<'PY'
import json, sys
expected = [l.split("\t")[0] for l in open(sys.argv[1]).read().splitlines()]
cands = json.load(open(sys.argv[2]))["meta"]["candidates"]
assert len(cands) == len(expected), (len(cands), len(expected))
want_keep = lambda e: e == "keep"
decided = [(e, c) for e, c in zip(expected, cands) if c["verdict"] != "unsure"]
agree = sum(1 for e, c in decided if (c["verdict"] in ("keep", "known")) == want_keep(e))
unsure = len(cands) - len(decided)
kept = [c for c in cands if c["verdict"] in ("keep", "known")]
kept_right = sum(1 for e, c in zip(expected, cands) if c["verdict"] in ("keep", "known") and want_keep(e))
print(f"notes {len(cands)}, decided {len(decided)}, agree {agree}/{len(decided)}, unsure {unsure} ({unsure * 100 // len(cands)}%), keep precision {kept_right}/{len(kept)}")
for e, c in zip(expected, cands):
    if "-v" in sys.argv[3:]:
        print(f"  {e:4} -> {c['verdict']:6} {c['reason']:6} score {c.get('score')} kind {c.get('kind')}")
    elif c["verdict"] != "unsure" and (c["verdict"] in ("keep", "known")) != want_keep(e):
        print(f"  disagree: expected {e}, got {c['verdict']} ({c['reason']}, score {c.get('score')}) note {c['index']}")
PY
