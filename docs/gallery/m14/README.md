# M14 gallery — Rivers, Lakes & Valleys

Seed `0x7e22ac0de`, default size (204.8 km). Maps from
`cargo test -p vox-worldgen --release world_map -- --ignored`, one pixel per
256 blocks (downscaled from the drainage grid's 128), palette-reduced. Views
are headless renders on lavapipe (`specks_scratch::specks`, noon, full
resolution daylit crudely by the harness) — not the owner's screenshots,
which come with the milestone's close. Every image made during the
milestone is kept here, with the commit it shows.

| Image | Commit | What |
|---|---|---|
| `rivers-before-erosion.png` | `c64c225` | The first network with closed basins, on the uneroded terrain: 78% of land drained to closed basins, every pit of the flat interior plateau its own salt flat (white specks). Sand tint: land draining to closed basins; cyan: lakes; teal: salt lakes; white: salt flats; blue lines: rivers by discharge |
| `rivers.png` | `688a5ab` | After valley erosion and the budget's fixes: 32% of land drains to closed basins, in the dry subtropical belts; lake districts on the ice-scoured bands; tarns in the ranges |
| `valleys.png` | `688a5ab` | The eroded ground, hill-shaded so the valleys read |
| `view-gorge.jpg` | `688a5ab` | A river-cut valley at about (143 300, 106 100), looking downstream (south-east) from 250 blocks up: V-shaped sides cut ~470 blocks deep at the deepest, opening to the lowland. No river water yet (task 3) |
| `view-first-river.jpg` | task 3 | The first river in the chunks (~10 m³/s, ~25 blocks wide) near (153 280, 36 800), looking upstream (north) from 60 blocks up: settled water in its channel, a tributary joining from the right. Past full resolution its bed shows dry (silt) — the LOD's water is task 5 |
| `view-frozen-lake-before-span-fix.jpg` | task 3 | A mountain lake (surface 756) from 300 blocks: its ice only in the chunks the streaming span happened to cover — the span followed the ground, 60 blocks below the lake's surface (a real bug; fixed, with a test) |
| `view-frozen-lake.jpg` | task 3 | The same lake from its shore after the fix: frozen over (mean annual temperature below −2 °C). Beyond the full-resolution disc the LOD shows its silt bed — the LOD's water is task 5 |
| `view-salt-flats.jpg` | task 3 | Salt flats near (87 744, 15 936) in a closed basin of the subtropical desert: white playa pans in the sand, badlands beyond |
| `view-frozen-lake-far-with-lod-water.jpg` | task 5 | The mountain lake from 300 blocks with the LOD's water (compare `view-frozen-lake-before-span-fix.jpg`): frozen over from full resolution into the LOD with no seam; an open lake far off on the right |
| `view-river-with-lod-water.jpg` | task 5 | The first river again (compare `view-first-river.jpg`): its water and its tributary's continue past full resolution into the LOD |
| `biomes.png` | task 4 | The 32 biomes: M13's 23 and the nine water biomes (river blue, lakes cyan, salt lakes white, floodplain bright green, marsh grey-green, swamp dark green, bog brown, delta tan, mangrove dark teal); lake districts on the ice-scoured bands. `rivers.png` updated with it |
| `view-river-flowing-through-marsh.jpg` | task 4 + flow map | The first river once more, now with its marsh (task 4: waterlogged ground at the river's level, shallow pools in mud) and its water drifting downstream (the flow map; a still frame — between the water clock at 0 s and 1 s, ~33 000 water pixels change and 10 others do) |
| `view-waterfall.jpg` | waterfalls | A 17-block waterfall near (112 704, 136 576), looking upstream from below: the sheet of falling water (its texture runs down, white water) between cliff walls, the river running on below. A waterfall is a step in a river's long profile: 12+ blocks, twice what the river below falls; ~1 900 on the default world |
| `view-delta-from-above.jpg` | deltas | The largest river's mouth (~11 m³/s) near (89 920, 197 312) from 260 blocks up: the river into the sea, its delta flats at sea level with pools and narrow distributaries, its marsh and sea-backed lagoons beside. Deltas here are small (~120 blocks): our catchments are small. The pale triangles toward the sea are the full-resolution/LOD boundary inside this wide top-down view |
| `view-frozen-oxbows.jpg` | oxbow lakes | A cold floodplain near (148 672, 61 504): crescents of ice (frozen oxbow lakes) beside the river's frozen floodplain lake, bog peat around |
| `bench-rivers-start.jpg` | bench water phases | Where `--bench`'s new rivers phases start on the default world, (−9 664, 32 064), 800×600 as the bench renders: the start near spawn whose sprint path crosses the most river and lake cells |
| `bench-waterfall-start.jpg` | bench water phases | The waterfall phase's view, (−13 796, 27 328): the largest waterfall near spawn falling toward the camera's side, its sheet in a mountain gorge |
| `walls-bench-waterfall-v4.jpg` | owner's walk: walls | The bench's waterfall view under generator v4, which the owner reported: the pool above the fall held up by an 85-block stone wall, a "flying river" (river shaping built the ground up to hold water standing above the full-resolution ground; the network's levels come from 128-block cells) |
| `walls-bench-waterfall-v5.jpg` | owner's walk: walls | The same view under v5: the river seated under its ground, the fall where the ground falls away, a gorge below it instead of a wall above |
| `walls-from-above-v4.jpg` | owner's walk: walls | The same place from 700 up, v4: the river on a raised spine with the wall at its foot; beyond, a frozen lake ending in a straight water wall 17 blocks over dry ground. (The darker square at lower left, in both, is the full-resolution area the headless renderer loads) |
| `walls-from-above-v5.jpg` | owner's walk: walls | v5: the river in a cut, its waterfall a gorge, the lake held by a shore |
| `bench-waterfall-start-v5.jpg` | owner's walk: walls | Where v5's bench waterfall phase now stands, (−13 720, 27 456): the falls moved, so it picks another; it now stands at least 12 above the ground (the hillside here is above the pool). The fall itself is behind the rise |
| `rivers-by-kind-v5.jpg` | owner's walk: walls | Six rivers near spawn under v5, 35 up: small torrent, large torrent, small meandering, large meandering at a coast, braided, wadi (left to right, top to bottom). Desert floodplains a block above flat ground with steps at their edges; the large river's channel and levees built out across the water as dikes |
| `rivers-by-kind-v6.jpg` | owner's walk: walls | The same six under v6: flats cut to their river's level and blended in, never raised; river shaping ends where a lake or the sea covers the ground; banks level with the water |
| `walk2-snow1-v6-v7.jpg` | owner's second flight | The owner's "white patches of snow", VIEW (−40 343, −75 808): v6 left, salt flats as squares on the 128-block cell grid; v7 right, crust on the ground near each closed basin's floor out to a wavy edge (commit e9fb581) |
| `walk2-snow2-v6-v7.jpg` | owner's second flight | (−47 069, −80 944): a salt lake and its flats, squares (v6) and contours (v7) |
| `walk2-falls-v6-v7.jpg` | owner's second flight | The owner's "insanely deep grooves", (−51 679, −82 227): v6, a slot one block wide and 13 deep below a lake; v7, the river stepping down its valley (long profiles, ADR-0020 §10) |
| `walk2-raised-v6-v7.jpg` | owner's second flight | (−51 769, −82 132): v6, a river on a ridge between vertical cliffs; v7, in its valley |
| `walk2-bridge-v6-v7.jpg` | owner's second flight | The owner's "straight line bridge", (−51 830, −81 758): v6, a river on a causeway over a hollow; v7, the hollow is a pond |
| `walk2-all-views-v7.jpg` | owner's second flight | All 13 of the owner's VIEW lines under v7, in order: snow ×2, falls, raised, bridge, salt lines, marsh ×2, bridge 2, steps, swamp, rivers merging, river into a lake |
| `walk2-rivers-merging-v7.jpg` | owner's second flight | The owner's "really cool view of a bunch of rivers merging", (−37 919, −99 379), swamp, v7 |
| `walk3-beach-v7-v8.jpg` | owner's third flight | "Funky stony beach", VIEW (−28 839, 22 141): a sea-level lake inland in a hot desert ringed with "rocky coast" stone (v7, left), coast being classed by height alone; v8, a sandy shore (coasts only within ~150 blocks of the sea) |
| `walk3-bridge3-v7-v8.jpg` | owner's third flight | "Still doing this weird bridging", (−43 985, 26 337): a dry river on a causeway across an inland basin below sea level that the sea's rule floods (v7); v8, the river ends at the water, which is a lake |
| `walk3-cutoff-v7-v8.jpg` | owner's third flight | "Fully cut off", (−52 712, 21 143): a braided river's gravel bars spanning it into pools (v7); v8, bars only in the middle, water along both banks |
| `walk3-break-v7-v8.jpg` | owner's third flight | "Weird break in river", (−51 131, 24 946): the same bars (v7); v8 |
| `walk3-ice-from-below-before-after.jpg` | owner's third flight | Distant ice seen from under a frozen lake, (−73 458, 26 453): v7 showed the sky through it, ruled by the LOD's border skirts; now an unlit underside and skirts only the ice's thickness (vox-mesh, no terrain change) |
| `walk3-pretty-view.jpg` | owner's third flight | The owner's "just a pretty view", alpine rock and snow, (−39 061, 25 840) |
| `walk3-riparian-strips-badlands.jpg` | owner's third flight | Green floodplain strips along rivers in badlands, (−42 432, 25 750): riparian corridors, as along desert rivers on Earth (the Nile, the Colorado) |
