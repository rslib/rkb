#!/usr/bin/env bash
# Builds the throwaway knowledge base for one run, from the fixture next to this script.
# `claude plugin eval --scaffold` runs it with HOME and the XDG_* folders in a temporary
# directory and the working directory at $HOME/cwd. rkb reads $HOME/Personal/kb by default,
# so every rkb path in the run already points here. See evals/README.md.
set -euo pipefail
# Never write the knowledge base into the real home.
real=$(eval echo "~$(id -un)")
[ "$HOME" != "$real" ] || { echo "setup.sh: HOME is the real home; refusing" >&2; exit 1; }
case "$PWD" in "$HOME"/*) ;; *) echo "setup.sh: $PWD is not under the temporary HOME" >&2; exit 1 ;; esac
kb=$HOME/Personal/kb
mkdir -p "$HOME/Personal"
cp -R "$(cd "$(dirname "$0")" && pwd)/kb" "$kb"
# Files a case needs in the working folder.
here=$(cd "$(dirname "$0")" && pwd)
[ -d "$here/work" ] && cp -R "$here/work/." "$PWD/"
git -C "$kb" init -q -b main
git -C "$kb" add -A
git -C "$kb" -c user.name=eval -c user.email=eval@example.invalid commit -q -m "fixture"
# /usr/bin/git is an Xcode shim that writes a cache in the real temp folder, which the agent's sandbox
# forbids, so `rkb add` fails on it. A wrapper in $HOME/bin runs the real git binary instead.
mkdir -p "$HOME/bin"
real_git=$(xcrun --find git 2>/dev/null || command -v git)
printf '#!/bin/sh\nexec "%s" "$@"\n' "$real_git" > "$HOME/bin/git"
chmod +x "$HOME/bin/git"
# The agent's shell reads this file. It pins every rkb path to the throwaway home, in case a profile
# sets RKB_HOME, and it puts the git wrapper first on PATH.
{
  echo "export RKB_HOME=\"$kb\" RKB_TEST_HOME=\"$kb\""
  echo "export XDG_STATE_HOME=\"$HOME/.local/state\" XDG_CACHE_HOME=\"$HOME/.cache\""
  echo "export XDG_DATA_HOME=\"$HOME/.local/share\" XDG_CONFIG_HOME=\"$HOME/.config\""
  echo "export PATH=\"$HOME/bin:\$PATH\""
} > "$HOME/.zshenv"
