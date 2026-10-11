---
type: regex
pattern: '\busr\b'
match: not_contains
target: {source: file, path: greet.py}
arm: both
---
