#!/bin/sh
test "$(sh build.sh 2>/dev/null)" = 1 && grep -q "regex" main.cpp
