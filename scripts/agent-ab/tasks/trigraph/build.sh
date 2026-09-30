#!/bin/sh
set -e
clang++ -std=c++14 -w main.cpp -o main
./main
