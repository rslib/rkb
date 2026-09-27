---
schema: 1
id: 2b00000051
type: pitfall
status: active
verified: 2026-02-01
verified_how: read
tags:
  - rust
  - borrow-checker
  - mutable-borrow
---

# The borrow checker rejects code that looks like it should compile

## Symptom
```text
error[E0502]: cannot borrow `v` as mutable because it is also borrowed as immutable
  --> src/main.rs:4:5
```
for code like:
```rust
let first = &v[0];
v.push(2);
println!("{}", first);
```

## Cause
`first` holds an immutable borrow of `v` that is still alive at the point of `println!`, and `v.push(2)` needs a mutable borrow; the two borrows' lifetimes overlap because `first` is used after the push, which the borrow checker must assume could reallocate `v` and invalidate the reference.

## Fix
End the immutable borrow before the mutable one is needed, typically by using the value sooner or copying it out:

```rust
let first = v[0]; // copy, not a reference, if the type is Copy
v.push(2);
println!("{}", first);
```

## Evidence
Changing `let first = &v[0]` to `let first = v[0]` (a `Copy` of an `i32`) removed the overlapping borrow and the code compiled without changing program behavior.
