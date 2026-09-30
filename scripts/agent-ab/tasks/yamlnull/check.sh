#!/bin/sh
set -e
test "$(python3 greet.py)" = "$(printf "Hello, ada\nHello, null\nHello, yes\nHello, grace")"
