---
schema: 1
id: 2b00000052
type: pitfall
status: active
verified: 2026-02-02
verified_how: read
tags:
  - rust
  - ownership
  - move
---

# A value cannot be used after being passed to a function

## Symptom
```text
error[E0382]: use of moved value: `data`
  --> src/main.rs:6:20
```
after code like:
```rust
process(data);
println!("{:?}", data);
```

## Cause
`process(data)` takes ownership of `data` by value (its parameter is not a reference), so calling it moves `data` into the function; once moved, the original binding is no longer valid to use unless the type implements `Copy`.

## Fix
Either pass a reference if `process` doesn't need to own the value, or clone it if a real duplicate is needed:

```rust
process(&data);              // if process only needs to read it
println!("{:?}", data);

// or, if process really must own its own copy
process(data.clone());
println!("{:?}", data);
```

## Evidence
Changing `process`'s signature to take `&Data` instead of `Data` let the caller use `data` again afterward without any clone, once it was confirmed `process` never needed ownership.
