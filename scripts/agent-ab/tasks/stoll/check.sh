#!/bin/sh
out=$(sh build.sh 2>err.txt); code=$?
grep -q "bad limit" err.txt && test "$code" = 1 && test "$(echo "$out" | head -1)" = 1500 || exit 1
printf 'timeout_s=2\nlimit=7\n' > settings.txt
test "$(sh build.sh)" = 2000
