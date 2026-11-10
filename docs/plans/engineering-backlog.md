# Engineering backlog

*Written 2026-11-09 from `docs/notes/code-audit-2026-11.md`. Work the audit
found and did not do, as packages a later session can pick up cold.*

**These are proposals, not decisions** — the same standing as
`docs/vision.md`. A package is built when the owner picks it: as a task in
the milestone under way, or as a milestone of its own (the larger ones say
so). The CLAUDE.md rules still bind every one of them — a spec before a
milestone, an ADR alongside any architectural choice, no new crate until a
spec calls for it, the gauntlet before every commit.

## How to use a package

1. Read CLAUDE.md, then the audit's finding the package names, then the
   package.
2. **Measure first.** Every performance package names the number it is
   meant to move and how to read it. Take the "before" on the commit you
   start from; if it is already fine, say so and stop.
3. Do the steps. Where a step says *failing test first*, write the test,
   watch it fail on the current code, then fix (CLAUDE.md, lesson 1).
4. A package is done when its **Done when** list holds — not when the code
   is written. Items marked **(owner)** need the game run on a real GPU.
5. Update the package's status line here in the same commit.

Sizes: **S** an hour or two, **M** a session, **L** several sessions (a
milestone, or most of one).

## Suggested order

Chosen so that each package makes the next cheaper or safer, and so that
none of it delays M15 by more than a session or two.

| When | Packages | Why then |
|---|---|---|
| **Now, before M15 task 1** | D1 (decide the heightmap rule), T1 (CI), T2 (toolchain) | Small. D1 removes a trap in the code M15 will not touch but every play-test exercises; T1 and T2 protect everything after |
| **Alongside M15** (it is all worldgen; these are all engine) | A1 (headless session), then P1 (async relight and mesh) on top of it | A1 is the enabling change: after it, engine work can be tested in the cloud. P1 is the largest remaining win for smoothness |
| **Before geology** | D3 (block names in saves), S4 (compact) | Geology adds the blocks that make D3 urgent |
| **A performance milestone after M15** (M12's successor) | P2, P3, P4, P5, P6, P7, P8 | GPU memory and fill rate: what a low-end machine needs. Wants its own spec and the owner's bench before and after |
| **Before block shapes / vegetation** | S5 (builds at distance), A2 (physics) | Both touch what those milestones touch |
| **Before anyone else plays** | D2 (world versions), S1 (region files), S2 (player state), S3 (panic save), A3 (loading screen) | The difference between a tech demo and a game someone can lose a world in |

---

## Decisions

### D1 — One heightmap rule for edits · S · needs the owner

**Finding.** CLAUDE.md says the skylight heightmap is raise-only; the code
lowers it on every edit (`App::recompute_column_height` →
`ColumnHeights::set`), and also runs a second, raise-only pass
(`App::recompute_height_column`) for the same edit. Audit finding D1.

**The choice.**
- **(a) Lowering is the rule** (what the code does). The height is the
  true top of the column's resident chunks. Light after a dig is right in
  one relight. Its risk is a rescan that misses a chunk — the audit fixed
  one such miss, and `resident_layers` now makes the scan exact.
- **(b) Raise-only is the rule** (what CLAUDE.md says). A dig never lowers;
  daylight reaches the new floor by propagation from the chunk above,
  over a few relights. Cannot invent daylight; converges more slowly.

Recommendation: **(a)**, because it is what has run since M09 without the
M05 symptoms returning, and because the scan is now exact. But it reverses
a written invariant, so it is the owner's call.

**Steps.**
1. Owner picks. Record it: amend CLAUDE.md's invariant, and note the
   reversal in a short ADR that supersedes the relevant part of ADR-0005.
2. Move the rescan into `vox-core` as one function on `ColumnHeights`
   (given the `World` and the registry) so it can be tested headless.
3. *Failing test first*, at the seam: a column whose top block sits in a
   resident chunk that the streamer's load window does not include (the
   keep-window case); break a block lower in the column; the height must
   not drop below that top. On the pre-audit code this fails.
4. Tests for the chosen rule: a shaft dug through two chunks is lit to its
   floor; a sealed side tunnel off it stays dark; a roof placed above an
   open column darkens it.
5. Delete the path not chosen. One function, one name, one doc comment.

**Done when.** One edit path; CLAUDE.md, `heightmap.rs` and the code agree;
the seam tests pass. **(owner)** Dig a shaft, roof it, open it: light is
right each time.

### D2 — What happens to a world when the generator changes · ADR · needs the owner

**Finding.** A world from another `GENERATOR_VERSION` is refused. Every
terrain change would cost every player their world. Audit finding D2.

**Options for the ADR.**
- **Pin and keep** — a world keeps the generator it was made with; the
  build ships old generators. Faithful; the code for every version must be
  kept and kept compiling.
- **Regenerate the untouched** — open it with the new generator; saved
  (edited) chunks keep their blocks, everything else regenerates. Cheap;
  leaves cliffs where old meets new, which is what the refusal was written
  to prevent.
- **Regenerate and blend** — as above, with edited regions blended into
  the new terrain over a margin. The most work; what players expect.
- **Refuse, but offer** — keep refusing by default; offer "open anyway" and
  "export my builds".

No recommendation: this depends on how the owner wants releases to feel.
Nothing to build until the ADR exists; needed before the first build
someone else plays.

### D3 — Block names in the save, not numbers · M · ADR

**Finding.** Chunks store numeric `BlockId`s, and the registry's order is
therefore the save format (`registry.rs`, *Stable IDs*). Audit finding D3.

**Steps.**
1. ADR: a world records a table of `name → id` as it was when the world
   was made; on load, ids in saved chunks are mapped through it to the
   current registry. Decide what an unknown name loads as (refuse; or a
   visible placeholder block).
2. `world.meta` version 3 (or a sibling `blocks.table`, versioned):
   written at creation, read at open. Version 2 worlds get the table of
   the registry as of M14, which is known exactly (ids 0–25).
3. `WorldStore::load_chunk` remaps palettes — a palette is a handful of
   entries, so this is free.
4. Tests: a world saved with one registry order loads correctly under
   another; a v2 world opens; an unknown name does what the ADR says.

**Done when.** Reordering `default_set()` in a test does not change what a
saved chunk loads as.

**Watch out.** `BlockId(0)` is air, permanently (CLAUDE.md); the table does
not move it.

---

## Performance

### P1 — Relight and mesh never block the frame · L · ADR · after A1

**Finding.** The frame thread hands relight and mesh batches to a pool and
waits (`relight_chunks_parallel`, `mesh_chunks_parallel`). Audit finding
P1; background in the audit's headline.

**Measure.** `--bench`, sprint phases: worst frame and 1% low; relight and
mesh milliseconds per second (wall) against `Relit /s × Relight µs/chunk`
and `Meshed /s × Mesh µs/chunk` (CPU) — the audit added the relight pair,
so "work" and "waiting" can be told apart.

**Design to propose in the ADR.** Jobs take what they read by value, run on
a pool, and return results over a channel that the frame drains — the shape
generation and LOD already have. A relight job needs its chunk's blocks,
six neighbour border planes and a heightmap plane: gather those on the
frame thread (cheap: planes are 1 KB each) and send them. A mesh job needs
the 34³ snapshot `MeshInput::build` already makes; build it on the frame
thread or send `Arc`s of the chunks (which means chunk data behind `Arc`,
copy-on-write on edit — decide in the ADR). Results that arrive for a chunk
that has since changed are dropped by a per-chunk generation counter.

**Steps.**
1. Record the before (the owner's bench on the audited build).
2. ADR. The invariants it must keep, each of which has a regression test
   that must stay green: never mesh a chunk queued for relight; a changed
   border re-meshes neighbours; the first-mesh gate; nearest-first.
3. Relight first (smaller snapshot), then mesh.
4. In A1's headless session, a soak test: a scripted flight where results
   are delivered late and out of order must end in the same lit, meshed
   world as the synchronous path. *This test is the reason to do A1 first.*

**Done when.** No `par_iter` on the frame thread; the soak test passes;
**(owner)** bench sprint worst-frame and 1% low improve, backlog still
drains, no dark chunks or seams in flight.

### P2 — Smaller vertices · M · ADR

**Finding.** `Vertex` and `LodVertex` are 36 bytes; indices are `u32`.
Audit finding P2.

**Measure.** The telemetry line's `MB gpu` at default settings (202 MB
today, ~190 of it LOD) and at 8 LOD levels (680–730 MB in the cloud run).

**Steps.**
1. ADR for the formats. Chunk vertex: position 3×6 bits (0–32), corner
   UV 2×6, layer 10, sky 4, block 4, shade 8 → two `u32`. LOD vertex:
   position needs more (node-local x/z 0–32 cells × stride, y a height):
   likely `i16`×3 or x/z as cell index and y as `f32`; morph target as an
   `i16` delta. Work it out from `mesh_lod_cells`' real ranges.
2. Decode in the vertex shaders; `every_shader_validates` already checks
   them with naga.
3. A round-trip test per format: pack, decode with the shader's arithmetic
   (mirrored in Rust, as the climate texel tests do), compare with the
   unpacked value for every representable input class.
4. `u16` indices where a mesh has under 65 536 vertices (two index formats,
   or split meshes — decide in the ADR).

**Done when.** Tests pass; **(owner)** the picture is unchanged (hole
census unchanged, a before/after screenshot pair in the gallery); `MB gpu`
falls by at least half.

**Watch out.** Greedy quads carry UVs up to 32 for tiling; shade is a
product of face brightness and AO — quantising it to 8 bits must not band
visibly. The geomorph (ADR-0009) needs the morph target's precision.

### P3 — Draw order · S

**Finding.** Opaque meshes drawn in hash order; transparent unsorted.
Audit finding P3.

**Steps.** In `Renderer::render`, collect the visible meshes of each pass
with their squared distance from the camera; sort opaque near-to-far and
transparent far-to-near; draw. Keep the telemetry counts as they are.

**Measure.** Bench fps on the owner's GPU; a weak or integrated GPU shows
it most. The sort is ~2 000 items a frame: measure `encode ms` does not
rise.

**Done when.** **(owner)** A lake seen through a waterfall, and a river in
front of a lake, blend correctly from both sides; fps not lower.

### P4 — Mipmaps for block textures · S

**Finding.** Audit finding P4.

**Steps.** Generate mip levels for the texture array at load (box filter,
per layer; water's alpha averaged too); sampler `mipmap_filter: Linear`,
keep `mag_filter: Nearest` for the blocky look up close; consider
anisotropy 4–8 for ground seen at a grazing angle. The tiles are generated
in `tile_pixels`, so there is no asset pipeline to change.

**Done when.** `only_water_is_translucent` and
`every_registry_layer_has_a_tile` still pass at every mip level;
**(owner)** distant ground no longer shimmers while walking; near blocks
look the same.

### P5 — Fewer GPU buffers · M

**Finding.** Three buffers and a bind group per mesh, reallocated per
re-mesh; every offset uniform rewritten on each render-origin move. Audit
finding P5; M12 measured the encode cost (~0.7 ms at 8 levels) and deferred
it, so **measure before starting** — do this only if `encode ms` or upload
hitches show in the bench on the target machine.

**Steps (in order of value).**
1. One uniform buffer for all offsets, indexed by a dynamic offset or a
   per-draw instance index: an origin move becomes one write.
2. Reuse a mesh's buffers when the new mesh fits (most re-meshes after an
   edit do).
3. Only if still needed: large shared vertex/index buffers with a
   free-list, and `multi_draw_indexed_indirect` where the adapter has it.

### P6 — Cheaper LOD nodes · M

**Finding.** ~7 ms per node at stride ≥ 4. Audit finding P6.

**Measure.** The audit's probe: time `Generator::lod_surface` per node at
each stride (it is 20 lines; rewrite it as an ignored `bench_` test in
vox-worldgen, as `bench_relight_realistic_surface_chunk` is in vox-core).

**Ideas, to be measured, not assumed.** `Exact` sampling at stride 4 reads
16 384 columns per node and shares none with the chunks generated beside
it: read through `GroundCache` where a node overlaps cached columns. Sparse
sampling calls the full `ground` 16 times per cell; most of a distant
cell's cost is river shaping it cannot show — a cheaper surface for cells
with no river within reach (the drainage map knows) may be most of the win.

**Watch out.** ADR-0008's never-exceed contract: a cell must not rise above
the real surface where LOD can meet full resolution. Any cheaper sampler
needs `lod_heightfield`'s existing tests plus one comparing it with `Exact`.

### P7 — A frame cap · S

**Finding.** Uncapped by default. Audit finding P7.

**Steps.** A setting: vsync (default on), or a frame-rate cap. `--bench`
forces uncapped, as it must. Needs S2 for the setting to survive a restart.

### P8 — Bench the build players will run · S

**Finding.** All numbers are dev builds. Audit finding P8.

**Steps.** A `[profile.release]` with `lto = "thin"`, `codegen-units = 1`;
decide `panic = "abort"` or not (abort loses S3's panic save). Run
`--bench` in both profiles once and record both in `docs/bench/`; from
then on quote release for "how fast is the game" and dev for "did this
change help".

---

## Safety of the player's work

### S1 — Region files, compression, a writer thread · L · milestone · ADR

**Finding.** One uncompressed 32 KB file per chunk, written synchronously
on the frame thread. Audit finding S1.

A milestone of its own: a chunk format v2 (bit-packed indices, then a
general compressor or run-length coding — measure both on real edited
chunks), region files of 16³ or 32³ chunks with an index, a writer thread
fed by a channel, and migration from v1 (v1 chunks must keep loading —
CLAUDE.md: never silently reinterpret). The LOD edit overlay needs the
same treatment: it is rewritten whole on every save and only grows.

Not urgent until gameplay modifies chunks the player did not touch (water,
growth, falling blocks). Needed before then.

### S2 — Save the player, the clock and the settings · M

**Finding.** Position, time of day and settings reset on every launch.
Audit finding S2.

**Steps.**
1. `player.dat` in the world folder, versioned (magic, version, then
   position as three `f64`, yaw, pitch, movement mode, selected block,
   world time in ticks). Written with the temp-and-rename the other files
   use, by `save_unsaved`.
2. On open: spawn there if the file reads and the position is inside the
   world's vertical bounds; else the existing spawn search.
3. Settings in a file beside the executable (they belong to the player,
   not the world), versioned, unknown or out-of-range values sanitised by
   `Settings::sanitize` — which already exists and is tested.
4. Tests: round-trips; a truncated or future-version file falls back
   without panicking; a position saved on a second lap of the torus loads
   as written (the player's frame never wraps — CLAUDE.md).

**Done when.** **(owner)** Quit on a mountain at dusk with the view
distance changed; start again; same mountain, dusk, view distance.

### S3 — Save on panic · S

**Finding.** Audit finding S3.

**Steps.** The save needs the `World`, which the panic hook cannot reach
safely. Simplest sound design: wrap the event loop's frame in
`catch_unwind`; on a caught panic, attempt `save_unsaved("panic")`, write
the panic message and backtrace to `crash-<time>.log`, then exit. Document
that a panic *inside* a save is not retried.

**Watch out.** If P8 chooses `panic = "abort"` for release, this cannot
work there; choose one.

### S4 — Compact chunks when they are saved · S

**Finding.** `Chunk::compact()` is never called. Audit finding S4; July
audit P1 #2.

**Steps.** In `App::save_unsaved` and the unload save, compact before
serialising (the chunk is already being written; the O(volume) pass is
off the hot path). A chunk that compacts to uniform must be re-lit and
re-meshed exactly as any edit is — or compact only the serialised form
and leave memory alone, which is simpler and gets the disk benefit; decide
by whether the in-memory fast paths matter for edited chunks (measure).

**Done when.** A chunk filled then emptied saves as 9 bytes, not 32 KB
(test in vox-core: serialise after compact).

### S5 — Builds visible at distance · L · ADR

**Finding.** The LOD edit overlay only lowers terrain. Audit finding S5.

**Why it is not a one-line change.** `EditedColumns` records one height per
edited column and `apply_to_node` takes the minimum with the seed's height,
because a dug column must never show the seed's higher ground. To raise, it
must know that the recorded top is the *whole* truth for the column
(replace, not minimum) — and a LOD cell covers many columns, so one tall
column in a cell of stride 32 must decide whether the cell rises (a spike
32 blocks wide) or not (the tower vanishes). It also needs the top block's
material, which the overlay does not store.

**Steps.** ADR first: what a build looks like from 2, 8 and 30 km — likely
"a cell rises when most of its columns are built up, and takes the most
common material", with thin towers left to a later impostor system. Then
overlay format v2 (height + block id), `apply_to_node` in replace mode,
and the tests `lod.rs` already has for lowering, mirrored for raising,
including the seam on the torus.

---

## Structure

### A1 — A headless world session · L · do before P1

**Status: done 2026-11-10** (steps 1 and 3; step 2's further split of
`main.rs` — camera, LOD pipeline, telemetry — is not, and is now optional).
`crates/vox-app/src/session.rs` holds `Session`; `main.rs` went from 3 561
lines to 2 558. The move was mechanical and the live run settles to the
same state as before (1 398 chunks, 256 meshes, 0.60 M triangles, 202 MB).
Five system tests, each checked against a deliberately broken engine:

| Test | What it holds | Breaks when |
|---|---|---|
| `a_flight_ends_lit_meshed_and_consistent` | after outrunning the loader, the streamer, world and heightmap agree chunk for chunk; everything is meshed; the light is a fixed point | a changed border stops re-queueing its neighbour |
| `arrival_order_does_not_change_the_world` | a saved world with a sealed room, a shaft and an overhang, approached with chunks prompt, or shuffled and late: identical blocks and light in every voxel; the room dark | same |
| `the_sea_surface_is_lit_whatever_arrives_first` | over deep ocean from high up (three separate groups of chunks per column), top-down, bottom-up and shuffled | a risen column stops relighting its stack |
| `a_deep_shaft_stays_dark_whatever_arrives_first` | a shaft far underground, open at the top of what is loaded, the surface arriving after it | that, or it together with "an unknown column is open" |
| `edits_across_a_seam_light_correctly_and_survive_a_restart` | a pit and a roof across a chunk border; then save, restart, and the same blocks and the same light | a changed border stops re-queueing its neighbour |

Two things learned writing them, both recorded in CLAUDE.md. Random
arrival order is a weak test: the engine heals most ordering mistakes when
some neighbour happens to arrive later, so the orders that matter are the
systematic ones nearest-first streaming produces — top-down for a camera
in the air, bottom-up for one underground. And "an unknown column is
covered" cannot be broken alone in any final state these tests can reach:
the stack relight repairs it. The two rules are a belt and braces, as the
M05 history says.

Not covered, honestly: the *transient* invariants (never mesh a chunk
still queued for relight — a flash and wasted work, not a wrong final
state), and anything about the LOD ring, which is still in `App`.

**Finding.** Everything that streams, lights and meshes the world is a
method on `App`, which needs a window; none of it is tested as a system.
Audit finding A1.

**Goal.** A type — call it `Session` — that owns the `World`, the
`Streamer`, `ColumnHeights`, `SpanCache`, the dirty/relight/meshed-once
sets, the generation channel and (later) the LOD ring, and exposes roughly:

    session.tick(camera_chunk) -> Vec<MeshUpdate>   // what the renderer must upload or drop
    session.break_block(pos) / place_block(pos, id)
    session.save_unsaved()

with no winit, no wgpu, no egui. `App` keeps the window, input, camera,
renderer and UI, and calls it. It lives in `vox-app` as a module for now
(CLAUDE.md: no new crate without a spec); it is the seed of the planned
`vox-client`/`vox-server` split and should be written with that in mind —
no rendering types in its interface.

**Steps.**
1. Move, do not rewrite. `stream_tick`'s five numbered steps become
   methods of `Session` with the same bodies; the renderer calls become a
   returned list. One commit per step, tests green at each. No behaviour
   change in this package — the live run before and after must end in the
   same state (the audit's method: same loaded/meshes/triangles/MB at
   spawn; see CLAUDE.md, *Cloud sessions*).
2. Split what is left of `main.rs` by responsibility while it is open:
   `camera.rs` (FlyCamera, physics), `lod_pipeline.rs`, `edit.rs`,
   `telemetry.rs`. Mechanical.
3. The tests this exists for, in `vox-app`:
   - **Soak:** fly a scripted path for N ticks with generation delivered
     synchronously; at the end every resident chunk is lit and meshed,
     no queue holds a non-resident chunk, `ColumnHeights` holds exactly the
     resident columns and layers.
   - **Order independence:** the same path with generation results
     delivered in a shuffled order and late; the final light of every
     chunk is identical to the in-order run. (This is the M05 class of
     bug, as a test.)
   - **Edits:** break and place across a chunk seam and at a keep-window
     edge; light equals a from-scratch relight of the same world.
   - **Save/load:** edit, autosave, drop the session, reopen: identical
     blocks.
   Use a small flat-ish world shape and a tiny load radius to keep each
   test under a few seconds.

**Done when.** `main.rs` no longer contains streaming logic; the four
tests exist and pass; the live run matches. **(owner)** One ordinary
play-test: nothing changed.

### A2 — Physics with a fixed step · M · after A1

**Finding.** Audit finding A2.

**Steps.**
1. Move `physics_update` and the AABB helpers into `camera.rs` (A1) as
   functions of a `World` reader, so they can be tested.
2. Fixed 60 Hz step with an accumulator; render interpolates between the
   last two states. *Failing tests first:* the same inputs at 30, 144 and
   1 000 fps end in the same position; a fall at terminal velocity onto a
   one-block floor lands on it at any frame time.
3. Sweep each axis move to contact instead of cancelling it (binary search
   or exact slab distance), so a fast fall stops on the ground rather than
   a frame above it.
4. Swimming (water is neither solid nor opaque — ADR-0011): buoyancy,
   drag, a slower fall. Small, but it is the first *feel* decision in the
   game; propose values and let the owner tune.

**Watch out.** CLAUDE.md says auto-step is for half-block heights and none
exist yet; leave `STEP_HEIGHT` alone until block shapes arrive.

### A3 — Show something while the world builds · S

**Finding.** `App::new` runs the stage-1 simulations before the window
opens. Audit finding A3.

**Steps.** Create the window and renderer first; run world open and the
stage-1 build on a thread; draw a plain progress screen (egui is already
in the frame) with the stage name until it finishes. A failed open
(`GeneratorMismatch`) becomes a message on that screen instead of a panic
in a console.

---

## Tooling

### T1 — Continuous integration · S

**Finding.** No CI. Audit finding A5.

A workflow is proposed at `docs/plans/ci-workflow.yml` (kept out of
`.github/` so adding it is the owner's choice — it uses Actions minutes on
a private repository). It runs fmt, clippy with `-D warnings`, and the
tests, on every push and pull request, on Linux. To adopt: copy it to
`.github/workflows/ci.yml`.

### T2 — Pin the toolchain, settle the edition · S

**Finding.** Audit finding A4.

**Steps.**
1. `rust-toolchain.toml` at the root with the version the owner runs
   natively (1.96 at the time of writing) and `components = ["clippy",
   "rustfmt"]`. Cloud sessions then use the same clippy as the gauntlet,
   and `docs/notes/clippy-lints.md` stops growing.
2. `edition = "2024"` in `[workspace.package]`, and `edition.workspace =
   true` in every crate. `vox-app` moves from 2021 to 2024: expect clippy's
   `collapsible_if` to ask for let-chains in a dozen places; fix them in
   the same commit.

### T3 — Faster worldgen tests · S

**Finding.** 63 s, mostly rebuilding identical stage-1 maps. Audit finding
A6.

**Steps.** A test-only `fn shared(seed, shape) -> &'static Generator`
backed by a `OnceLock` map; tests that only read use it. Tests that need a
fresh generator (cache tests, determinism across instances) keep building
their own.

### T4 — Dependency upgrades · M

wgpu, egui and winit move together (see the pin comment in the root
`Cargo.toml`). Do it as its own change, between milestones, with a
before/after bench and the hole census — never in the same commit as
rendering work.
