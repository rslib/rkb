#!/bin/sh
set -e
head -1 count.zsh | grep -q zsh
d=$(mktemp -d); cp count.zsh "$d"; cd "$d"
test "$(zsh count.zsh)" = "0 logs"
touch a.log b.log
test "$(zsh count.zsh)" = "2 logs"
