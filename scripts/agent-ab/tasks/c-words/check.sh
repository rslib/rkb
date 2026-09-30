#!/bin/sh
test "$(sh count.sh notes.txt)" = 5 && printf "a b\n" > t.txt && test "$(sh count.sh t.txt)" = 2
