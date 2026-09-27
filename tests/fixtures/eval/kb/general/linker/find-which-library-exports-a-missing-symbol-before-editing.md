---
schema: 1
id: 2b00000009
type: recipe
status: active
verified: 2026-09-04
verified_how: ran
tags:
  - linker
  - nm
  - ldd
---

# Find which library exports a missing symbol before editing the link line

## When to use
When a link fails with `undefined reference` and it is unclear which of several available libraries actually provides the symbol.

## Steps
Search each candidate library for the symbol's mangled or plain name:

```sh
nm -D --defined-only /usr/lib/libfoo.so | grep foo_init
for a in *.a; do echo "== $a =="; nm "$a" | grep foo_init; done
```

For a running binary, `ldd ./app` lists which shared libraries actually resolved, which narrows down whether the problem is at link time or load time.

## Evidence
`nm` on the candidate archives showed the symbol only in `libfoo_static.a`, not the `libfoo.a` already on the link line, which explained the failure without guesswork.
