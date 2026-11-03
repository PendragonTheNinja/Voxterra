# M13 gallery — Climate & Biomes

Seed `0x7e22ac0de`, default size (204.8 km). Planet maps from
`cargo test -p vox-worldgen --release world_map -- --ignored`, one pixel per
256 blocks; the dotted lines are the equators, tropics and polar circles,
and the sea is dimmed on the climate maps. Views are headless renders on
lavapipe (`specks_scratch::specks`, 960×540, 8 LOD levels, noon) — not the
owner's screenshots, which come with the milestone's close.

| Image | What |
|---|---|
| `before-elevation.png` | The planet before M13: every land column the same grass |
| `biomes.png` | The 23 biomes of `docs/biomes.md` |
| `temperature.png` | Mean annual temperature at the ground |
| `precipitation.png` | Annual precipitation |
| `view-coast-steppe.png` | The benchmark's start: hot steppe (yellowed grass, sand and coarse-dirt patches), a sand beach, badlands and snow-capped peaks far off |
| `view-steppe-near-and-far.png` | The same steppe with full resolution (daylit crudely in the harness): grass colour continues into the LOD with no seam |
| `view-rainforest.png` | Equatorial rainforest (trees come with vegetation) |
| `view-desert.png` | The subtropical desert: sand, gravelly cold desert on raised ground, red badlands |
| `view-ice-cap.png` | The polar ice cap, sea ice at its coast |
