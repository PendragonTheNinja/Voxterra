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
