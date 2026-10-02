#!/usr/bin/env bash
# Type-checks the pi and omp extension in extensions/pi against the pi that is installed here, so an API
# change in a pi update fails here before a session does. omp ships one binary with no type files, so it
# is covered only through pi's types.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
src="$repo/extensions/pi"
agent="${PI_AGENT_DIR:-$HOME/.pi/agent}"
version=$(cat "$agent/install/current-version" 2>/dev/null) || {
  echo "no managed pi install in $agent; checked against the pinned package only" >&2
  exec npm --prefix "$src" run --silent check
}
modules="$agent/install/releases/$version/node_modules"
[ -d "$modules/@earendil-works/pi-coding-agent" ] || { echo "pi $version has no pi-coding-agent package in $modules" >&2; exit 1; }
[ -x "$src/node_modules/.bin/tsc" ] || (cd "$src" && npm install --silent)

dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
mkdir -p "$dir/src" "$dir/node_modules/@earendil-works" "$dir/node_modules/@types"
cp "$src/src/rkb.ts" "$dir/src/"
cp "$src/tsconfig.json" "$dir/"
for p in "$modules/@earendil-works/"*; do ln -s "$p" "$dir/node_modules/@earendil-works/"; done
ln -s "$modules/typebox" "$dir/node_modules/typebox"
ln -s "$src/node_modules/@types/node" "$dir/node_modules/@types/node"
read_from=$("$src/node_modules/.bin/tsc" -p "$dir" --listFiles | grep -c "releases/$version/" || true)
[ "$read_from" -gt 0 ] || { echo "tsc did not read pi $version's types" >&2; exit 1; }
"$src/node_modules/.bin/tsc" -p "$dir"
pinned=$(node -p "require('$src/package.json').devDependencies['@earendil-works/pi-coding-agent']")
echo "pi extension: typed against installed pi $version (package.json pins $pinned)"
