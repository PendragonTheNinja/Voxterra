# ADR-0020: Water above sea level — rivers and lakes in the chunks

- **Status:** Proposed (2026-11-01, M14 task 3). To be accepted after the
  owner's walk.
- **Context:** Until M14 the generator's fill rule was one line: every block
  above the ground up to sea level is water (ADR-0011). Rivers and lakes
  stand at their own levels, hundreds of blocks above the sea, and the
  water decision (the hybrid, `docs/design/water.md`) fixes how they are
  stored: full, settled blocks, no per-block water state, reservoirs known
  to the network. Stage 2 must stay a pure function of `(x, z)` so chunks,
  the LOD and every readout agree, and any chunk generates alone.

## Decision

### 1. The column carries its water

`Generator::ground(x, z)` returns the surface and a `Water { level, body }`:
a river (with its bed: gravel, sand or silt by slope), a lake, a salt
lake, a salt flat, or dry. `Column` carries it; `materials::block_in`
fills **water from the ground up to `level`** — the river's or lake's own
level, not sea level — freezes a lake's top block where the sea would
freeze (below −2 °C mean annual), and lays the bed: river gravel, sand or
silt two blocks deep; silt under lakes; a salt crust over salt flats and
salt lakes' beds. The sea's rule is unchanged beneath it.

### 2. Rivers: channels from the network, settled water in whole steps

`DrainageMap::shape_column` (`drainage::rivers`). Each cell carrying 0.01+
m³/s is a reach from its (jittered) centre to its receiver's; a column
looks at the reaches of the 5×5 cells around it (cached per thread by
cell: columns come in runs). The query point is warped by meander noise
(`Elevation::meander_warp`, octaves of 32–1 024 blocks blended) at ~12×
the river's width, amplitude growing as the slope falls, plus a fixed
grid-scale wander; width `8·√Q` (1–~40 blocks), depth `2.8·Q^⅜` (1–12),
interpolated along the reach so a tributary meets its main river where the
main river's bends put it. The water surface is the network's channel
level interpolated along the reach and floored to a whole block: it steps
down the long profile and never rises downstream.

### 3. The ground conforms: beds, banks, levees, fills

- In the channel the ground is the bed, parabolic across.
- Beside it, within 48 blocks, the ground lies between a **ceiling** (a 45°
  rise from a block above the water) and a **floor** (a levee: a block
  above the water beside it, allowing for a steep reach's fall), both
  easing out to the natural ground; where two reaches disagree the levee
  wins. **Water is never held by air**: tested over ~20 000–46 000 wet
  columns of every size of river and lake, with steps of two or more
  blocks only where a river steps down its own profile.
- A hollow a river's grade runs above — a rim it could cut no more than 8
  blocks through — is **filled with sediment** to that grade (a flat
  alluvial floor), unless it backs up from a lake, when it is the lake's.
- Lakes fill every column of their cells, and their neighbours', whose
  ground is below their level: the shore follows the ground's contours.

### 4. Streaming loads the water

`Generator::surface_span_chunks` — the one sampler streaming, the
first-mesh gate and LOD coverage share — spans standing water as well as
ground: a lake's surface stands tens of blocks above its bed, in layers
the ground alone never loaded (found in a render; tested). Chunk
generation's all-air shortcut likewise reads the water's top.

### 5. Lighting needs nothing new

Water is rendered but neither solid nor opaque (ADR-0011): skylight and
the skylight heightmap ask `opaque`, so a lake's bed is lit through it
exactly as the seabed is, and the lighting invariants hold unchanged.

### 6. Water that flows

River water drifts downstream at its speed (M14 criterion 10), in the
shader, from a **flow map**: the network's velocity per cell
(`DrainageMap::flow` — Manning's relation, `v = R^⅔·S^½ / n` with the
channel's depth and n = 0.035, 0.05–4 m/s; cells beside a river take its
flow, lakes and the sea none), encoded as RG8 (`vox_render::encode_flow`)
and bound beside the climate map, which it shares placement and sampler
with (ADR-0018's pattern). `flow.wgsl`, appended to both terrain shaders,
samples water's tile twice along the flow half a cycle apart and
cross-fades them (the usual flow-map technique: no stretching, forever),
with foam where it is fast; water's top samples by world position, so near
and far tile alike. The water clock is real time, not world time: rivers
keep flowing when the day stands still. Re-meshes nothing; one texture
sample more per water pixel. Visual only, so it may use `cbrt`.

### 7. Rivers differ by kind

Each reach is one of Earth's kinds (`drainage::river_kind`), from its
slope, discharge and the sediment its climate gives it — the cover byte
carries whether a cell is arid (precipitation under half its potential
evaporation) or ice-scoured (cache format 2):

- **Wadi** — arid and under 0.05 m³/s: a dry sand channel, no water.
- **Braided** — 0.002–0.03 slope, arid or ice-scoured, 0.05+ m³/s: 2.5×
  as wide, a block deep, split by dry gravel bars (32-block noise).
- **Torrent** — steeper than 0.01: gravel bed, fast, nearly straight.
- **Meandering** — the rest: sand, or silt under 0.002, winding, slow.
- **Slow, lake-like reaches** are where a river crosses a lake.

Speed (§6) follows from slope and depth for all of them.

### 8. Waterfalls: the one place water stands against air

A reach that falls 12+ blocks, and at least twice what the river below
it falls — a step in the long profile, not merely a steep one — drops all
at once at its middle: the pool above keeps its level to the lip, a sheet
1.5 blocks deep past it holds water from the lower river's bed up to the
upper level, and the banks beside it stand at the upper level a little
further, a cliff. The sheet's downstream face meets only the lower water,
so the never-held-by-air test still guards everything else (it accepts
water stepping down to lower water). In settled water (the hybrid's layer
1) the sheet is static; flow.wgsl runs every water side face's texture
straight down, white with foam, so it reads as falling (owner's decision,
2026-11-01, until layer 2 makes water fall). ~1 900 on the default world.

The terrain fingerprint now also samples columns at rivers of every kind,
waterfalls, lakes and closed basins chosen from the network: scattered
columns almost never landed on water, and a waterfall change had moved no
pin (checked: disabling waterfalls now fails it).

### 9. Rivers sit in their ground (v5, the owner's walk)

The owner's first flight found rivers on walls: "raised blocks around them
not congruent with the terrain, especially bad in the mountains", and at
the bench's waterfall "a massive wall of raised stone that supports
basically a flying river" — 85 blocks of it under the pool, which §8 held
level to the reach's midpoint over ground falling steadily below it. The
cause throughout: levels come from 128-block cells' centres, and the
full-resolution ground under a reach is often lower, so §3's levee built
it up. `GENERATOR_VERSION` 5:

- **Seated reaches** (ADR-0019 §2.6): stage 1 lowers each stream until
  its line runs a block under the lowest ground across its belt (half its
  width, its meanders' amplitude and the grid-scale wander, either side).
  A river crossing a hillside sits in a cut, not on a ridge.
- **Waterfalls** fall where the ground falls away below the pool (the lip
  stored per cell) and at most 32 blocks; below, the river runs at the
  fall's foot until its grade comes down to it — a gorge as deep as the
  fall.
- **Fills** raise their own cell flat and slope down at 45° through their
  neighbours' low ground, instead of flattening it all to the fill's level
  and ending in a cliff where the ground beyond was lower.
- **Lakes** reach 48 blocks into their neighbours' cells; beyond, a bank
  falling at 45° holds them (a lake stood 17 blocks over dry ground at a
  straight cell edge). A higher lake beside a lake's cells is held the
  same way.
- **Banks** blend into the land by smoothstep over BANK_REACH from the
  nearest channel, not by a quartic term that outgrew the ground and stood
  deep cuts' sides up as cliffs.
- **Floodplains and wetlands** are laid only on ground within 2 blocks
  below their river's level.

Version 6 (renders of every kind of river near spawn):

- **Flats are cut, never built**: floodplains, wetlands and deltas lower
  ground to their level, blending out over their reach and 4 blocks above
  it; ground below is left alone (a block-high strip with stepped edges
  along every desert river).
- **Rivers end at still water**: no channel or bank is shaped where a lake
  or the sea covers the ground (a reach runs on to the water's cell
  centre, and its levees had stood out across it as dikes).
- **Banks only where needed**: a lake's bank stands only where no water as
  high — the sea's included — covers the column; fill slopes stay under a
  lake or the sea; river banks are level with the water for a block, then
  fall at 45° (a bank a block above the water made every flat river a
  little levee).

Tests that fail on earlier versions: `rivers_build_no_walls`,
`lake_shores_hold_their_water` (v4), `rivers_end_at_still_water_and_flats_are_cut` (v5).
Rivers in filled hollows still run above the hollow's natural floor — on
its sediment, which now meets its sides.

### 10. Long profiles; regions as discs (v7, the owner's second flight)

A reach's water no longer runs a straight line from node to node. Stage 1
stores each falling reach's **long profile**: its level at 8 points, each a
block under the lowest ground across its belt as far as the next point,
never rising downstream, between its own level and its receiver's. Stage
2 slopes the water from point to point and drops a step of 12+ at once —
a waterfall where the ground steps. (The straight line cut slots where the
ground was concave and stood on causeways where it was convex; seated
against it, a lake's outlet sank 27 blocks below the lake.) Nodes are
seated only against the ground near them, never against ground a lake or
the sea covers. Where a river is held at its receiver's level over a
hollow, ground within 24 blocks below its water is a pond, not a causeway.

A cell's lake, fill or salt flat reaches a **disc** about its centre
(radius 0.9 cell ± 0.15 by noise), not its square: no straight shores,
square terraces or square salt pans. Fills are applied before a river's
banks are cut; a lake's bank stands only where the lake's own edge on that
side is below its level; lake water only where the natural ground is below
it; a river's banks keep a lake's rim; under a lake its channel runs on
(bed and sides cut, nothing built). Wetland pools count water as high as
ground beside them. Drainage cache format 4.

## Consequences

- `ground` costs ~2× the eroded surface per column (~0.6 against ~0.3 µs in
  a release build, noisy): chunk generation and LOD sampling pay it. The
  per-thread reach cache halved it; `--bench` will say whether more is
  needed.
- **The LOD shows the same water** (task 5): `Generator::lod_surface`
  returns each cell's water — the top of the water standing on the lowest
  column it sampled, the column that sets its ground — and
  `vox_mesh::mesh_lod_cells` draws the translucent surface at each cell's
  own level, the sea's generalised (ADR-0011 decision 1c). A frozen lake
  rises to its ice as a polar sea does. Tested: at stride 1 every cell's
  water is its column's. Small streams fade with distance on their own:
  sparse sampling rarely lands in a one-block channel.
- The fill rule is a single function (`materials::block_in` with the
  column's `Water`), as the water design requires, so the simulation (layer
  2) can ask what "settled" means for any column.
- New blocks: silt, salt, river gravel (peat comes with bogs, task 4).
