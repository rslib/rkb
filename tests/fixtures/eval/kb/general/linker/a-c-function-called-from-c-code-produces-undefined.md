---
schema: 1
id: 2b00000007
type: pitfall
status: active
verified: 2026-09-02
verified_how: read
tags:
  - linker
  - name-mangling
  - extern-c
---

# A C++ function called from C code produces undefined reference errors

## Symptom
A C translation unit that calls a function implemented in a `.cpp` file fails to link with:

```text
undefined reference to `process_frame'
```

even though `nm` on the object file shows a very similar-looking symbol.

## Cause
C++ compilers mangle function names to encode argument types (`nm` shows something like `_Z13process_framePKc`), while C code and callers compiled as C expect the plain, unmangled name. Without `extern "C"` around the declaration, the two sides look for different symbol names.

## Fix
Wrap the C++ side's declaration (and definition, or just the declaration in a shared header) in `extern "C"`:

```cpp
extern "C" void process_frame(const char *data);
```

This tells the C++ compiler to emit the plain C symbol name so the C caller's reference matches.

## Evidence
After adding `extern "C"` to the header shared between the `.c` and `.cpp` files, `nm` showed matching symbol names on both sides and the link succeeded.
