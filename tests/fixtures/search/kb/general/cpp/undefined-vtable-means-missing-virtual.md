---
schema: 1
id: 1a00000012
type: pitfall
status: active
verified: 2026-09-25
verified_how: read
tags:
  - linker
---

# Undefined reference to vtable means a missing virtual definition

## Symptom
The link fails with "undefined reference to `vtable for Widget'".

## Cause
The class declares a virtual function, often the destructor, that no source file defines.

## Fix
```cpp
Widget::~Widget() = default;
```

## Evidence
The link passed after the destructor was defined in the source file.
