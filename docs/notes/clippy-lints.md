# Clippy lint reference (sandbox blind spots)

**Why this file exists.** Clippy cannot run in Claude's sandbox (no `rustup
component add`, not in apt), and the sandbox toolchain is older than the
project's Rust 1.96 / edition 2024. So every clippy failure lands on Nathan's
native run and costs a round-trip. This is the accumulated list of lints that
have actually broken a build here, so Claude can scan for them before handing
off. **Add to it every time a new one bites.**

Read this before handing off any Rust changes. It is not a substitute for the
native `cargo clippy --workspace --all-targets -- -D warnings`, which stays
authoritative.

---

## The subtle one: clippy WANTS edition-2024 syntax

`collapsible_if` on a nested `if let … { if cond { … } }` asks for a
**let-chain**:

```rust
// clippy wants this (edition 2024):
} else if let Some(&worst) = heap.peek()
    && key < worst
{

// NOT this:
} else if let Some(&worst) = heap.peek() {
    if key < worst {
```

This is a trap for Claude specifically: the sandbox's rustc **cannot compile**
let-chains, but the real repo requires them. Procedure — write the let-chain in
the real file, and shim it back to nested `if` **only in the `fixcheck` copy**
to run the tests. Same class of thing as the pre-existing let-chain in
`chunk.rs`. Never let a shim reach the real file.

---

## Lints that have actually broken this build

| Lint | What triggered it | Fix |
|---|---|---|
| `collapsible_if` | nested `if let … { if … } }` | let-chain (see above) |
| `neg_multiply` | `-1 * 8 * CHUNK_SIZE` | `-(8 * CHUNK_SIZE)` |
| `iter_cloned_collect` | `slice.iter().copied().collect()` | `slice.to_vec()` (only for **slices** — collecting from a `HashSet`, or *into* a `HashSet`, is fine) |
| `needless_range_loop` | `for i in 0..4 { out[i] = f(a[i]) }` | iterators, `zip().enumerate()`, or `std::array::from_fn` |
| `identity_op` | `0.0 + (0.5 - 0.0) * t` | simplify the arithmetic |
| `unnecessary_cast` | `(TICKS_PER_DAY / 4) as u64 as u64` | drop the redundant cast |
| `manual_range_contains` | `x >= a && x <= b` | `(a..=b).contains(&x)` |
| `doc_lazy_continuation` | wrapped doc-comment bullet without indent | indent continuation lines |
| `duplicated_attributes` | repeated `#[derive]`/attr | merge them |
| `dead_code` | leftover binding from a refactor (e.g. `line_axis`) | delete it — don't paper over with `let _ =` |
| `manual_div_ceil` | `((x + n - 1) / n)` on **unsigned** ints | `x.div_ceil(n)` — but ONLY for unsigned. `div_ceil` on **signed** ints (`i64`) is still unstable (`int_roundings`), so signed code must keep the manual form. Applying clippy's suggestion blindly to an `i64` produces a compile error. |
| `too_many_arguments` | `emit_rect` (11 args) | genuinely warranted here → explicit `#[allow(clippy::too_many_arguments)]` with a reason |

---

## Pre-handoff checklist

Before sending Rust files, grep the diff for:

1. `-1 *` or `* -1` → `neg_multiply`
2. `.iter().copied().collect()` / `.iter().cloned().collect()` on a slice →
   `to_vec()`
3. `for i in 0..N` that only indexes → `needless_range_loop`
4. nested `if let { if }` → let-chain (`collapsible_if`)
5. `as T as T`, `+ 0`, `* 1` → `unnecessary_cast` / `identity_op`
6. `x >= a && x <= b` → `manual_range_contains`
7. bindings introduced then unused mid-refactor → `dead_code`
8. new multi-line doc comments → `doc_lazy_continuation`
9. `x.div_ceil(n)` on a SIGNED integer → unstable; use `((x + n - 1) / n)`

Also: run `cargo test` in `fixcheck` (catches real compile errors, never
lints), and remember the sandbox cannot compile `vox-render`/`vox-app` at all —
those are review-only, so their lints are found exclusively on the native run.
Extra care there.
