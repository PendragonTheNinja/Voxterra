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

## Consequences

- `ground` costs ~2× the eroded surface per column (~0.6 against ~0.3 µs in
  a release build, noisy): chunk generation and LOD sampling pay it. The
  per-thread reach cache halved it; `--bench` will say whether more is
  needed.
- **The LOD does not show river or lake water yet** — it draws their beds.
  Task 5 (distant water) generalises the LOD's sea surface to a per-cell
  water level.
- The fill rule is a single function (`materials::block_in` with the
  column's `Water`), as the water design requires, so the simulation (layer
  2) can ask what "settled" means for any column.
- New blocks: silt, salt, river gravel (peat comes with bogs, task 4).
