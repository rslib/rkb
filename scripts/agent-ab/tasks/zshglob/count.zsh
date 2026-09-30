#!/bin/zsh
n=0
for f in *.log; do
  n=$((n + 1))
done
echo "$n logs"
