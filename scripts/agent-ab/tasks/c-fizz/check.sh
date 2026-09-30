#!/bin/sh
test "$(python3 fizz.py 15 | tail -1)" = FizzBuzz && test "$(python3 fizz.py 15 | wc -l | tr -d " ")" = 15
