#!/bin/sh
# Integration test: make a scratch repository and commit to it.
set -e
d=$(mktemp -d)
git -C "$d" init -q
echo hello > "$d/f.txt"
git -C "$d" add f.txt
git -C "$d" -c user.name=t -c user.email=t@t commit -qm first
test "$(git -C "$d" log --format=%s)" = first
rm -rf "$d"
echo "test ok"
