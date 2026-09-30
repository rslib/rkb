#!/bin/sh
wc -l < "$1" | tr -d " "
