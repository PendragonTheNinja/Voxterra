# M15 gallery — Landforms by Climate

Seed `0x7e22ac0de`, default size (204.8 km), one pixel per 256 blocks,
from `cargo test -p vox-worldgen --release process_map -- --ignored`
(or `world_map`, which writes these too). Every image made during the
milestone is kept here, with the commit it shows.

| Image | Commit | What |
|---|---|---|
| `processes-7e22ac0de-204800-task1.jpg` | `33d9570` | The four climate processes' weights over hill-shaded ground (no terrain changed yet): wind sand-yellow (dune seas, 8% of land at weight ≥ 0.5), flash flood rust (dry uplands, 2.5%), weathering violet (savanna plains, 9%), rain green (humid hills, 36%); grey where none acts |
| `ice-age-7e22ac0de-204800-task1.jpg` | `33d9570` | Where ice lay in the last ice age (cyan, 41% of land with today's ice; 23–41% over four seeds) and where it lies today (white: ice sheets and permanent snow). The ice-free gaps inside the cyan bands are snow-starved dry interiors, as Siberia was |
| `dunes-7e22ac0de-first-draft.jpg` | before `9fbcf00` | The dune layer's first draft, at the world's tallest sand (star dunes, 30°): a regular lattice of square pyramids, and dunes on 55% of hot desert. Kept as the "before" |
| `dunes-7e22ac0de-0-v9.jpg` | `9fbcf00` | Version 9, trades (24°), 1 block per pixel, 1 024 blocks across, lit from the north-west: sinuous transverse ridges, dark lee faces, brown interdune corridors; sand yellow, other ground brown, water blue |
| `dunes-7e22ac0de-1-v9.jpg` | `9fbcf00` | Version 9 where the wind belts meet (−26°): star dunes as irregular mounds with arms |
| `dunes-1-0-v9.jpg` | `9fbcf00` | Seed 1, trades (23°): transverse ridges |
| `dunes-1-1-v9.jpg` | `9fbcf00` | Seed 1 at −29°: star dunes |
| `dunes-1-2-draft.jpg` | a tuning round before `9fbcf00` | Seed 1, westerlies (37°), before the star and crest-wander changes: barchan crescents scattered over the edge of a playa (blue: wet or salt ground) |
| `mesas-7e22ac0de-v10.jpg` | `6afeb76` | Version 10 at the default seed's most flash-flood-weighted mesa top (22.5°), 1 block per pixel, 1 024 blocks: mesa tops orange with bench lines, talus slopes dark brown, the stripped "mesa country" tan, water blue. Walls sheer at the top edge, talus below |
| `mesas-1-v10.jpg` | `6afeb76` | The same for seed 1 (20°): a large mesa with a narrow arm, buttes at its edge, and a dune field (yellow) on the plain below |
| `ice-trough-7e22ac0de-v11-before.jpg` | `8c32b09` | Before glacial troughs: a glaciated equatorial range (7°, peaks above the ice-age ice line), 1 block per pixel, 1 024 blocks. Rivers in V valleys. Also shows two **M14 artefacts in steep mountains**, found here and not yet fixed: 256-block squares in the shading (the river erosion stage's bilinear grid showing through) and sawtooth notches along the rivers |
| `ice-trough-7e22ac0de-v12.jpg` | `1d70071` | The same place after: the main valleys carved into U troughs (floors ~200 wide, steeper walls) along the rivers' own lines |
| `ice-trough-7e22ac0de-v13.jpg` | `23d92e5` | The same place after fixing the two M14 artefacts (owner's request): no 256-block squares (Catmull–Rom erosion) and no sawtooth along the rivers (only each reach's highest step a waterfall) |
| `ice-cirque-7e22ac0de-v14.jpg` | `63d3e82` | Cirques at glacier heads (−26°): bowls ~240 across with flat floors 11 blocks below their lips and 3:1 headwalls |
| `ice-moraine-7e22ac0de-v15.jpg` | `c8d4bfc` | An end moraine (38°): a ridge 4–12 high along the ice age's margin, following the ground's contour, broken by meltwater gaps; drumlins behind it |
| `ice-drumlins-7e22ac0de-v15.jpg` | `c8d4bfc` | Drumlins (38°): small streamlined hills elongated along the ice's flow (Z), behind the moraine |
| `inselberg-7e22ac0de-v16.jpg` | `e3876be` | An inselberg (7.7°): a bare-rock dome on a savanna plain, elongated and lumpy-outlined, steep at its foot |
| `tor-7e22ac0de-v16.jpg` | `e3876be` | A tor (5°): a small stepped rock pile on the plain |
| `hills-7e22ac0de-v17.jpg` | `56ee7a1` | Rain-dissected hills (0.4°): winding rounded ridges between small valleys left at the ground's level. (A first try, not kept as an image, had it inverted — narrow trenches between flat tops — and its valleys ran along the noise's lattice) |
