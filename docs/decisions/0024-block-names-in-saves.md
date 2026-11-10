# ADR-0024: A world remembers its blocks by name

- **Status:** Accepted in principle by the owner (2026-11-10: "If this is
  important and good for the game in the long run, I'm all for it"); the
  design below is proposed and not yet built. Backlog package D3; audit
  finding D3. **To be built before geology adds its blocks.**
- **Context:** A saved chunk stores numeric `BlockId`s, and a `BlockId` is
  an index into the registry's list (`vox_core::registry`). The list's
  order is therefore part of the save format, protected only by a comment
  ("STONE = 1 … unchanged ON PURPOSE"). Today there are 26 blocks and one
  author. Geology brings rock types by the dozen and ores by the dozen
  more, added over months, grouped and regrouped as the design settles; a
  single insertion in the middle of the list renumbers everything after
  it, and every saved world then loads with granite where its player laid
  sandstone — silently, because every id is still a valid block.

## Decision

1. **Every block has a permanent key**: a short lowercase identifier
   (`stone`, `red_sand`, `river_gravel`), separate from its display name.
   The display name may change; the key never does. A block that is
   renamed in the design keeps its key, or declares the old key as an
   alias.
2. **A world keeps its own table of `id ↔ key`**, in `blocks.table` in the
   world folder: versioned like every other file (magic, version, then
   each entry's id and key), written with the temp-and-rename the others
   use.
3. **Chunk files store ids in the world's numbering, not the build's.**
   The table only ever grows: when a build knows a block the world's
   table does not, the key is appended with the next free id, and the
   table is rewritten before any chunk using it is saved. An id, once
   given to a key in a world, means that key in that world for ever.
4. **Chunks are translated at the save layer** — the one place that
   already translates positions (the torus seam, ADR-0012): on load, a
   chunk's palette is mapped world → build; on save, build → world. A
   palette is a handful of entries, so this costs nothing measurable, and
   when the two numberings are identical (every world, until the registry
   is first reordered) it is skipped outright. Nothing above the save
   layer ever sees a world id.
5. **A world with no table is a world from before this ADR**, whose ids
   are the registry's as of M14 — ids 0 to 25, known exactly and written
   into the code as that table. It is given the file on first open.
6. **A key the build does not know stops the world from opening**, with a
   message naming the keys. A block is never removed from the game
   without an alias to what replaces it, so this can only happen when an
   old build opens a newer world — which must not be allowed to save over
   blocks it cannot represent.
7. **Air is id 0 in every world and every build** (CLAUDE.md); the table
   records it and nothing may move it.

## Consequences

- The registry's order stops being a format. Blocks can be grouped by
  kind, and the list can be generated from data later (the modding API on
  the roadmap needs exactly this table).
- `BlockId` values in memory remain dense indices into the build's
  registry, so nothing in meshing, lighting or generation changes.
- Generated terrain is unaffected: the generator names blocks by the
  registry's constants, and unedited chunks are never saved.
- One more file per world, and one more thing a corrupt world can lack —
  handled like the LOD edit overlay: reported, never silently replaced,
  because a wrong table is worse than a missing one.

## Tests it must come with

- A world saved under one registry order loads block-for-block under a
  shuffled one.
- A pre-ADR world (no table) opens, gets its table, and reads the same.
- A build with a new block appends it; an older table's ids are unmoved.
- An unknown key refuses, by name; an aliased key loads as its successor.
- The identity case does no per-chunk work (the palette is not rebuilt).

## Alternative considered

**Names in every chunk file.** Self-describing and needs no table, at the
cost of a string per palette entry per chunk and a format that cannot be
read without string comparison. The table gives the same safety once per
world.
