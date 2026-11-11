# M16 gallery — Tectonics & Bedrock

Seed `0x7e22ac0de`, default size (204.8 km) unless a row says otherwise.
Every image made during the milestone is kept here, with the commit it
shows.

| Image | Commit | What |
|---|---|---|
| `tectonics-7e22ac0de.jpg` | task 1 | The tectonic framework beside the terrain it does not yet place (`cargo test -p vox-worldgen --release tectonics_map -- --ignored`, one pixel per 256 blocks). **Left, the framework:** each plate its own colour — pale where it carries continent, palest on cratons, dark where it is ocean floor; a white line from each plate's centre is its drift; a ringed white dot with a tail is a hotspot and its track. Boundaries by kind: **red** collision, **orange** continental arc, **yellow** island arc, **cyan** spreading ridge, **magenta** rift, **black** transform, **cream** passive margin, **brown** sutures, thin **grey** seams. **Middle, what task 2 will read:** continental crust tan and ocean blue (`continentalness`, clamped 3 500 blocks either side of the coast) with the belt envelope in white — arcs along the active margins, old welds wrapping the cratons, island arcs at sea, nothing on ridges, transforms or passive coasts. **Right:** today's terrain (generator version 8), placed by noise; the two do not agree yet, and making them agree is task 2. The map wraps on all four edges |
| `tectonics-7e22ac0de-smallest.jpg` | task 1 | The same for the smallest legal world (32 768 blocks, one pixel per 82): nine terranes on three plates |
| `task2-tectonics-7e22ac0de.jpg` | task 2 | The same three panels once the framework places the terrain (generator version 9): the right panel now follows the left — ranges on the belts, trenches as dark lines off the arcs, ridges as pale lines through the oceans |
| `task2-tectonics-7e22ac0de-smallest.jpg` | task 2 | The same for the smallest world |
| `task2-basin-close-up.jpg` | task 2 | A 20 km square (`WORLD_MAP_AT=113000,33000,20000`): a terrane ringed by arcs and welds is a closed basin, filled flat to its spill level against straight range fronts — rhombic, like the Tarim and Qaidam basins |
| `v8-v9-elevation-7e22ac0de.jpg` | task 2 | The default world's terrain, generator version 8 (left) and 9 (right): one large continent placed by noise against a dozen placed by plates |
| `v8-v9-biomes-7e22ac0de.jpg` | task 2 | Its biomes, version 8 and 9 |
| `v8-v9-rivers-7e22ac0de.jpg` | task 2 | Its rivers, version 8 and 9 (half resolution) |

