# ADR-0021: An edit sets a column's height to its true top

- **Status:** Proposed (2026-11-10), for the owner's decision. Resolves
  finding D1 of the 2026-11 code audit. If accepted it supersedes the
  "raise-only" rule for edits in CLAUDE.md's lighting invariants (added
  after M05); chunk *arrival* stays raise-only, exactly as it is.
- **Context:** The skylight heightmap (ADR-0005) holds, per block column,
  the highest opaque block among resident chunks. Two things write it:

  - a chunk **arriving**, which can only raise it — chunks of a column
    stream in any order and the highest wins;
  - a block **edit**.

  CLAUDE.md says of edits: "The heightmap is raise-only (edits/digging
  never lower it) … a too-high height only makes columns darker than
  truth; a too-low height invents daylight." The code has not done that
  since M09. Every break and place runs a raise-only pass
  (`recompute_height_column`) and then a rescan that sets the height to
  whatever the column's top now is, lower included
  (`recompute_column_height` → `ColumnHeights::set`), added with the M09
  "ghost block" fix. Two functions with near-identical names, one rule
  written down and the other running.

## The evidence

Until the audit this could only be argued. The world pipeline is now
testable without a window (`vox-app/src/session.rs`), so both rules were
run against the same tests.

| Test (`session::tests`) | Edits lower (the code) | Edits raise-only (CLAUDE.md) |
|---|---|---|
| A pit and a roof across a chunk seam: light is a fixed point, and equals the light a restart computes for the same blocks | pass | pass |
| A sealed room, a shaft and an overhang, approached in any arrival order | pass | pass |
| The sea surface from high above; a deep shaft under unloaded rock | pass | pass |
| **A block placed high above the ground, removed, then the player comes down** (`removing_a_high_block_returns_daylight_to_the_ground`) | pass | **fail** |

The last is the case that separates them. Once the chunks that held the
block have unloaded, the heightmap is the only thing that knows what is
above the column — and under raise-only it still says "a block, up
there". The column's chunks are left holding light that their own
inputs no longer justify, and the ground keeps the shadow of something
that no longer exists for as long as any chunk of the column stays
loaded. A dismantled tower, a removed roof over a wide area seen from a
low camera, a scaffold: every building game does this constantly.

Raise-only is safe in the direction it was written to protect (it cannot
invent daylight) and wrong in the other.

## Decision (proposed)

**An edit sets the column's height to the highest opaque block among the
chunks resident in that column — up or down.** Chunk arrival is unchanged.

What made lowering dangerous in M05 was not the rule but the scan: a
height set from a scan that missed a chunk is lower than the truth, and a
too-low height does invent daylight. The audit found one such miss (the
scan read the layers the streamer *loads*; it *keeps* one more) and fixed
it by scanning the heightmap's own record of the column's resident chunks
(`ColumnHeights::resident_layers`), which is exact by construction: the
heightmap is told of every chunk that comes and goes.

One window remains, and it closes itself: an edit made while a chunk
higher in the same column is still streaming in sets the height without
it. When that chunk arrives it raises the height, and a risen column
relights its whole stack (CLAUDE.md; held by
`the_sea_surface_is_lit_whatever_arrives_first` and
`a_deep_shaft_stays_dark_whatever_arrives_first`).

## Consequences, if accepted

- CLAUDE.md's invariant becomes: *arrival raises; an edit sets to the true
  resident top; neither may ever set a height from a scan that can miss a
  resident chunk.* The "UNRESOLVED" note added by the audit is removed.
- The two edit functions become one, in `vox-core` on `ColumnHeights`
  (given the world and the registry), with the seam test from the
  backlog's D1 package: a top block in a chunk held only by the unload
  hysteresis must be seen.
- `recompute_height_column`'s second job — queueing the column's chunks
  and their horizontal neighbours for relight after an edit — is kept; it
  was never about the height.

## If rejected

Then the code must change to match CLAUDE.md: remove the `set` call, and
accept that the test above fails — or find another way for a column to
forget a block above it that is gone (for instance, dropping a column's
height whenever a chunk of it unloads and rebuilding it from what is
left, which is a rescan by another name).
