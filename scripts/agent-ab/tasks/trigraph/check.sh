#!/bin/sh
grep -q -- "-std=c++14" build.sh && test "$(sh build.sh)" = "Is this right??)"
