#!/usr/bin/env bash
# Validates, type-checks and tests the Claude Code mod in extensions/claude against the installed `claude`.
# Claude Code writes a mod's types only when it loads the mod, so the mod is loaded once in print mode
# with a slash command that runs no model.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
src="$repo/extensions/claude"
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

mkdir -p "$dir/.claude-plugin"
cp -R "$src/hooks" "$src/types" "$src/tests" "$dir/"
# The real plugin's skill, so a mod name that clashes with it fails here.
mkdir -p "$dir/skills/rkb" && cp "$repo/skills/rkb/SKILL.md" "$dir/skills/rkb/"
sed -i.bak 's#__RKB__#rkb#' "$dir/hooks/register.tsx" && rm "$dir/hooks/register.tsx.bak"
printf '{ "name": "rkb", "version": "0.0.0", "description": "rkb mod check", "types": "./types/index.d.ts" }\n' >"$dir/.claude-plugin/plugin.json"
printf '{ "modules": ["./register.tsx"] }\n' >"$dir/hooks/hooks.json"

out=$(claude plugin validate "$dir" 2>&1) || { echo "$out"; exit 1; }
(cd "$dir" && RKB_OBSERVER= claude -p --plugin-dir "$dir" --debug-file "$dir/debug.log" "/cost" >/dev/null)
if grep -E "refused|hook failed|skipped:" "$dir/debug.log" | grep -i "rkb"; then
  echo "Claude Code refused part of the mod; see the lines above" >&2
  exit 1
fi
test -f "$dir/.claude-plugin/types/tsconfig.json" || { echo "claude did not write the mod's types" >&2; exit 1; }
[ -x "$src/node_modules/.bin/tsc" ] || (cd "$src" && npm install --silent)
"$src/node_modules/.bin/tsc" -p "$dir"
claude plugin test "$dir"
echo "claude mod: valid, typed and tested with $(claude --version)"
