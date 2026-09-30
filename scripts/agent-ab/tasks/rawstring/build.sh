#!/bin/sh
set -e
clang++ -std=c++17 main.cpp -o main
./main
