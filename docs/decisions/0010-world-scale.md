# ADR-0010: World scale — mirror Earth's systems, not Earth's dimensions

- **Status:** Accepted (M10). Supersedes the scale figures in the first draft of
  `docs/milestones/10-terrain-and-deep-vertical.md`; the milestone's structure,
  bounds and vertical range are unchanged.
- **Context:** M10 shipped an elevation field built to literal Earth
  proportions — one block one metre, ocean basins 4 300 blocks deep, peaks
  reaching 8 300. It generated correctly, passed every statistical test written
  for it, and was **unplayable**.

## What actually happened

The owner spawned, flew for several minutes, and found: a colossal smooth green
wall, an enormous dead-flat plain, and one small hill. Three separate things had
gone wrong, and only one of them was obvious.

1. **The "mountain" was not a mountain.** It was the continental margin seen
   from the abyssal plain — 4 300 blocks of ocean depth with no water rendered
   to hide it. Water is not cosmetic at this vertical scale; it conceals the
   single largest piece of relief in the world.
2. **The world held almost nothing.** Every field was tuned in tens of
   kilometres — relief varying over 27 km, mountain belts over 56 km — in a
   200 km world. That is a handful of features in total.
3. **Landforms had no interior.** Detail amplitude on a massif was 3.5% of its
   height, so an 8 000-block mountain rendered as a smooth dome. Real ranges are
   made of peaks, spurs and valleys at 1–5 km.

## The measurement that settles it

| | Earth | Voxterra |
|---|---|---|
| Pole to pole | 20 000 km | 200 km |
| Surface area | 510 000 000 km² | 40 000 km² (~Switzerland) |
| Land area | 149 000 000 km² | ~9 200 km² (~Lebanon) |
| Tallest peak | 8.85 km | was ~8.3 km |
| **Width : peak height** | **2 260 : 1** | **was 24 : 1** |

Everest's massif — the Himalaya — covers 600 000 km², **fifteen times the
entire world**. Literal Earth relief does not merely look wrong at this size; it
does not fit by an order of magnitude.

## Decision

**Keep the 200 km world. Mirror Earth's systems; scale its dimensions.**

Growing the world was rejected: making it wide enough for literal Earth
landforms means a world that takes days to cross on foot, and the owner's
position is that widening without a matching increase in height is incoherent
anyway. Shrinking relief to match the world's width was also rejected — at
1:200 the tallest peak on the planet would be 83 blocks, which is a hill.

**Amplitude and wavelength are separate knobs, and neither is set by a ratio to
Earth. They are set by WALKING TIME.**

That reframing is the substance of the decision. A first correction divided the
vertical by 5 and the horizontal by 3 and was still wrong, because it was judged
by flying at spectator sprint — a speed no survival player will ever have. At
5.6 m/s, the speed players actually move, wavelengths of tens of kilometres mean
an hour of walking between landforms.

The field is therefore tuned against what a player meets on foot:

| Feature | Wavelength | Sprint-walking time |
|---|---|---|
| Detail underfoot | 800 | 2.4 min |
| Terrain character (relief) | 2 600 | 7.7 min |
| Crossing a range | 7 000 | 21 min |
| Reaching the next mountain belt | 13 000 | 39 min |
| Crossing a landmass | 34 000 | 101 min |

Amplitudes then follow from those wavelengths, because slope is amplitude over
wavelength and slope is what decides whether ground is traversable: peaks to
~1 150, ocean 260, trenches to ~600, lowlands ~70. Massif interior detail rose
from 3.5% to 13% of a massif's height, so a range is a range and not a dome.

Proportions within a landform stay Earth-like — a 1:3 flank is still 1:3, ranges
still run in lines, rain shadows will still work in M11. What changed is
density: 200 km now holds a continent's worth of variety rather than one
continent's corner.

Measured on the same seed, at the median land start: a 3-minute walk now spans
42 blocks of elevation, a 9-minute walk 94, a 30-minute walk 176.

## Consequences

- **`terrain_changes_as_you_travel`** is now a test. Nothing else in the suite
  caught the failure — a world can be spectacular and boring at once, and every
  statistical property (land fraction, flatness, rarity of high ground, gradient
  distribution) was *passing* while the world was unplayable. The test measures
  elevation spread across a 10 km walk at the median land start; it reads 134
  blocks now against 83 at the wavelengths it replaced.
- **Judge terrain at walking speed, not flying speed.** Both wrong tunings
  survived review because they were assessed from a spectator camera at four
  times sprint. Flight compresses an hour of walking into a minute and makes an
  empty world look merely large.
- **Every threshold in the elevation tests is now a fraction of the field's own
  constants**, not an absolute block height. Scale has changed once and may
  change again; what must hold at any scale is the *shape* of the distribution.
- **The deep vertical range stays open** (−11 264 … +9 216) even though
  generation now uses a fraction of it. It costs nothing — surface-following
  streaming means world height is free — and it is what a simulation-scale
  variant would need. That work is not wasted, only unused.
- **Water becomes load-bearing, not decorative.** At 420 blocks the ocean is
  shallow enough to read correctly once filled, but until it is filled the world
  still presents as basins and walls.
- **This is a game, not a simulation.** Recorded plainly because the pull toward
  literal realism is strong and was followed once already. The rule going
  forward: mirror the *mechanisms* — orogeny in belts, rain shadows, climate by
  latitude, erosion following drainage — and choose the dimensions to serve
  play.
