# M15 gallery — Landforms by Climate

Seed `0x7e22ac0de`, default size (204.8 km), one pixel per 256 blocks,
from `cargo test -p vox-worldgen --release process_map -- --ignored`
(or `world_map`, which writes these too). Every image made during the
milestone is kept here, with the commit it shows. Rows up to `hills-…-v17`
were made on the landform line's own terrain (versions 9–17, before the
tectonic framework); the two lines merged as version 18.

| Image | Commit | What |
|---|---|---|
| `process-weights-7e22ac0de.jpg` | task 1 (main, 2026-11-10) | Where each landform process acts (`cargo test -p vox-worldgen --release process_map -- --ignored`, one pixel per 256 blocks). Land is grey, darker with height, tinted by each process's weight: **gold** wind (dune seas), **red** flash flood (dry plateaus), **pale blue** ice of the last ice age, **violet** deep weathering (inselberg plains), **green** rain (dissected hills). The poles are at one and three quarters of the way down (latitude loops, CLAUDE.md). Dune seas are patches in the subtropical deserts' basins, 20–24% of hot desert; ice covers the mountains everywhere and all land poleward of ~47°; the sea shows the ice age's reach faintly, for fjords. Superseded by the line below: two task 1s were built in parallel sessions, and the landform module's won the merge (2026-11-14) |
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
| `processes-7e22ac0de-204800-v18.jpg` | `eaf76d4` | **Version 18, the landforms on the tectonic terrain** (the merge). The process weights: wind on 4.7% of land at weight ≥ 0.5 (8% on the old terrain), flash flood 2.6%, ice 39.5%, weathering 7.3%, rain 51% (36%) — the tectonic continents are wetter |
| `ice-age-7e22ac0de-204800-v18.jpg` | `eaf76d4` | Version 18: where ice lay in the last ice age; glaciated land by 10° band from the equator 2, 2, 1, 13, 93, 99, 86, 99, 100% |
| `dunes-7e22ac0de-0-v18.jpg`, `dunes-7e22ac0de-1-v18.jpg` | `eaf76d4` | Version 18 dune seas (20.6°, −30.5°): transverse ridges and star dunes as before; dunes on 21–30% of hot desert over four seeds; 0.4–1.6% of steps between dune columns over a block (under 0.5% on the old terrain: the ground under them is rougher) |
| `mesas-7e22ac0de-v18.jpg`, `mesas-1-v18.jpg` | `eaf76d4` | Version 18 mesas: the layer on 8–10% of land, tops ~1%. **No river of 0.1+ m³/s crosses flash-flood ground in any of three seeds** — so no canyons (one to four cells a world on the old terrain) |
| `inselberg-7e22ac0de-v18.jpg`, `tor-7e22ac0de-v18.jpg`, `hills-7e22ac0de-v18.jpg` | `eaf76d4` | Version 18: an inselberg (1°), a tor, dissected hills (22.8% of land, 16.5% on the old terrain) |
| `ice-moraine-7e22ac0de-v18-before.jpg` | `eaf76d4` | **Before (version 18):** over the tectonic terrain's flat lowland the ice weight follows latitude, and the moraine ran dead straight for kilometres; the drumlins behind it stood on a grid of identical hills. Also a round crater (top left): a cirque cut 80 deep into a hillside gentler than that |
| `ice-moraine-7e22ac0de-v19.jpg` | v19 | **After (version 19):** the margin bent into lobes by a slow noise (`moraines_bend_in_lobes`), the drumlins staggered strip by strip, each its own size, a quarter left out (`drumlins_are_not_a_grid`), the cirque no longer a crater |
| `ice-drumlins-7e22ac0de-v18-before.jpg`, `ice-drumlins-7e22ac0de-v19.jpg` | `eaf76d4`, v19 | Drumlin swarms before and after (different places: the survey picks the tallest) |
| `ice-cirque-7e22ac0de-v18-before.jpg`, `ice-fjord-7e22ac0de-v18-before.jpg` | `eaf76d4` | **Before:** cirques as round bowls walled all round on all but the steepest slopes — 256 of 1 952 had no way out lower than 12 blocks above their floors; the fjord's coast dotted with them |
| `ice-cirque-7e22ac0de-v19.jpg`, `ice-fjord-7e22ac0de-v19.jpg`, `ice-trough-7e22ac0de-v19.jpg` | v19 | **After:** a cirque is cut no deeper than the slope below it falls across the bowl, plus a tarn's 6-block lip (`cirques_open_downhill`): armchairs opening into their troughs. The trough (cut 548, −16.5°) and fjord (cut 184, 47°; 26 on the old terrain) |
| `valley-hills-7e22ac0de-v19-before.jpg` | `736bf16` | **Task 5, before (version 19):** a 2 m³/s river among dissected hills (56°, `valley_closeups`), its valley the erosion grid's, ~30 below the ground 192 blocks either side, ponds stacked along it |
| `valley-hills-7e22ac0de-clipped-draft.jpg` | a draft of v20 | The first inner valleys, clipped to a U about straight node-to-node lines: deeper, but the ridged hillside sliced into flat facets with straight edges. Not kept |
| `valley-hills-7e22ac0de-v20.jpg` | v20 | **After (version 20):** the inner valley lowers the ground instead, fading out over its sides — the ridges run down to the floor; no stacked ponds. Across it every 16 blocks: 84 … 25~ … 113 |
| `valley-large-7e22ac0de-v20.jpg` | v20 | Version 20, the default world's largest river well up from its mouth (9.5 m³/s, −4°). Rivers of 1–10 m³/s now sit ~30 blocks below the ground 384 blocks off (median, three seeds; 18–19 at version 19, 11–13 at M14's close) |
