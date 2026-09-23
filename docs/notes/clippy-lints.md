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
| `manual_range_contains` (negated form) | `ix < 0 \|\| ix >= cells` — a bounds *reject*, not a bounds accept | `!(0..cells).contains(&ix)`. The lint fires on both polarities; the reject form is easy to miss when scanning for the `>= a && <= b` shape in the checklist. |

---

## Pre-handoff checklist

Before sending Rust files, grep the diff for:

1. `-1 *` or `* -1` → `neg_multiply`
2. `.iter().copied().collect()` / `.iter().cloned().collect()` on a slice →
   `to_vec()`
3. `for i in 0..N` that only indexes → `needless_range_loop`
4. nested `if let { if }` → let-chain (`collapsible_if`)
5. `as T as T`, `+ 0`, `* 1` → `unnecessary_cast` / `identity_op`
6. `x >= a && x <= b` → `manual_range_contains`, and its negation
   `x < a || x >= b` → `!(a..b).contains(&x)`
7. bindings introduced then unused mid-refactor → `dead_code`
8. new multi-line doc comments → `doc_lazy_continuation`
9. `x.div_ceil(n)` on a SIGNED integer → unstable; use `((x + n - 1) / n)`

Also: run `cargo test` in `fixcheck` (catches real compile errors, never
lints), and remember the sandbox cannot compile `vox-render`/`vox-app` at all —
those are review-only, so their lints are found exclusively on the native run.
Extra care there.

---

## Grep the sandbox build output for WARNINGS, not just failures (2026-08)

`cargo test` in `fixcheck` catches compile errors, but the native gauntlet runs
`-D warnings`, so a plain rustc warning there is a hard error on Nathan's
machine. The sandbox emits it in the same output — it is only missed by
filtering the output too narrowly.

Cost one round trip in M09/A4: an `unused_mut` on a test-helper closure
(`let mut set = |h: &mut Vec<i32>, ...|` — the closure takes its buffer as a
PARAMETER, so it captures nothing and needs no `mut`). `cargo test` reported it
as a warning and passed; `-D warnings` rejected it.

**Before any handoff, run `cargo build --workspace --all-targets` in `fixcheck`
and confirm the output is empty apart from `Compiling`/`Finished` lines.**
`--all-targets` matters: the failing line was in a `#[cfg(test)]` module, which
a plain `cargo build` never compiles. Note this only covers the three headless
crates — `vox-render`/`vox-app` warnings still surface only on the native run.

---

## "Unused padding" is a promise that expires (2026-08)

`ChunkData.offset.w` was commented `// .w unused padding (16-byte align)` and
`set_render_origin` accordingly wrote `.w = 0.0` when rewriting `.xyz`. When A4
gave `.w` a meaning (each LOD node's geomorph completion distance), that write
became a silent feature-killer: the render origin moves every time the camera
crosses a chunk, so morphing switched itself off across the whole world within
32 blocks of walking. No error, no warning — the feature simply did nothing
whenever it mattered, and read as "I can't tell the difference".

**When taking over a field previously marked unused/reserved/padding, grep for
every writer of the whole struct, not just the field.** A partial write that
was correct while the field was padding becomes a clobber the moment it isn't.
Here the fix was to store `.w` alongside the mesh (`GpuMesh::offset_w`) so any
rewrite carries it through.

The same shape applies to `SkyUniformData::params.yzw` and the sky-pass `.w`
slots — all still genuinely unused, all written wholesale in `set_sky`. Fine
today; a trap the day one of them means something.

---

## wgpu validation is a RUNTIME panic, not a compile error (2026-08)

The review-only crates hide a second class of error that no amount of reading
Rust catches, because it is not Rust: wgpu validates shaders against the
pipeline layout at `create_render_pipeline`, i.e. on startup, in a panic.

M09/A4 hit this: `lod.wgsl` reads the sky uniform (group 3) in its VERTEX stage
for the geomorph centre and band, but `sky_bgl` was declared
`ShaderStages::FRAGMENT` — correct for every previous shader, since only
lighting and fog read it. Result:

    In Device::create_render_pipeline, label = 'lod pipeline'
      Shader global ResourceBinding { group: 3, binding: 0 } is not available
        Visibility flags don't include the shader stage

### Check when writing or changing any WGSL

For every `var<uniform>` / texture / sampler the shader reads, confirm the
matching `BindGroupLayoutEntry.visibility` includes **every stage that reads
it**. Adding a stage to `visibility` is free and never breaks another pipeline
sharing the layout — wgpu only rejects a stage that is MISSING, never one that
is spare. Current state, for reference:

| Group | Binding | Visibility | Read by |
|---|---|---|---|
| 0 | camera | `VERTEX` | both vertex stages |
| 1 | chunk/node offset | `VERTEX` | both vertex stages |
| 2 | block texture + sampler | `FRAGMENT` | both fragment stages |
| 3 | sky/fog/morph | `VERTEX_FRAGMENT` | fog+light in fragment, geomorph in `vs_lod` |

Also worth knowing: a uniform BUFFER larger than the WGSL struct that reads it
is legal (the sky buffer grew to 4 vec4s while `shader.wgsl` only needed 3), so
that mismatch will NOT be caught. Keep the struct declarations in sync by hand.

---

## The review-only boundary is dependency-driven, not fixable (2026-08)

`cargo check --workspace` in the sandbox does not fail on missing system
libraries — it fails in **dependency resolution**, before compiling anything:

    feature `edition2024` is required
    ... wayland-protocols-0.32.13/Cargo.toml

Cargo 1.75 (the newest Ubuntu packages) cannot parse manifests from the
winit/wgpu tree. So `vox-render` and `vox-app` stay review-only no matter what
gets installed, and no amount of shimming reaches them. Don't spend time
retrying this.

### Consequence: a pre-handoff grep for every changed shared type

The compiler cannot catch a type change that crosses into the review-only
crates, so do it by hand. **When a public type or signature in `vox-core` or
`vox-mesh` changes, grep the whole repo for the OLD name before handing over**
— not just the call sites already being edited.

Learned the hard way in M09/A4: `mesh_lod_heightfield` changed from `MeshData`
to `LodMeshData` and both direct call sites were updated, but the mpsc channel
carrying the result between the rayon job and the main thread was still typed
`Sender<(LodNodeId, MeshData)>`. Two compile errors on Nathan's machine that a
single `grep -n MeshData crates/vox-app/src/main.rs` would have found. The
generic-looking places (channels, `HashMap` values, struct fields, `Vec<_>`
collections) are the ones that hide a type name far from the code being
changed.

---

## What the sandbox CAN install (2026-08)

`apt-get install -y rustc cargo rustfmt` works (Ubuntu 24.04 ships 1.75) —
so the `fixcheck` workspace no longer needs a hand-rolled toolchain, and
**formatting is now checkable in the sandbox**. `clippy` is still not
packaged (`E: Unable to locate package clippy`) and `rustup` is still
unreachable, so this file remains necessary.

Two cautions when using the sandbox `rustfmt`:

1. **Run it on a COPY, diff, and port only the width-driven changes.** Copy
   the file (plus any sibling `mod` targets, or it fails to resolve them) to
   a scratch dir, `rustfmt --edition 2021`, then diff.
2. **Ignore its import ordering.** 1.75 sorts `{ChunkPos, CHUNK_SIZE}`;
   edition-2024 style sorts `{CHUNK_SIZE, ChunkPos}`. Taking its suggestion
   there would churn every file and lose on Nathan's run. Everything else it
   changes is `max_width`/`chain_width`/`fn_call_width` wrapping, which is
   edition-independent and safe to port.
