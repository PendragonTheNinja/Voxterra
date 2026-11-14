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

| `flight1-*.jpg` | task 2 (`e4b4beb`) | The owner's first flight on version 9 (2026-11-11), re-rendered headlessly from his `VIEW` lines (`specks`, full resolution, lit): **1** beach sand and a rocky-coast stone band following the shore's contours as smooth parallel curves; **2** shallow-sea sand meeting deep-ocean mud in one hard line on a steep slope; **3, 4** torrents running down mountain hillsides in straight one-block channels; **5** a river's banks built across a lake at exactly the lake's level, standing as green lines on the water; **6** the near LOD's colour and edge against full resolution; **7** a rift lake at sea level, 34 deep, its full-resolution water clear and its LOD water opaque |
| `polish-2-seabed-before-after.jpg` | polish 2 | The owner's view 2 before (left: shallow-sea sand against deep-ocean mud in one line) and after (right: sand, a patchy silt band, then mud) |
| `polish-3-coast-view1-before-after-above.jpg` | polish 3 | The owner's view 1 before (left) and after (middle) the coast's material began alternating — unchanged here, a headland, so rocky — and the same coast from above (right). The bands parallel to one smooth shoreline remain: they are the coast's shape, proposed as a landform family for M15 |
| `polish-4-lod-slope-before-after.jpg` | polish 4 | The owner's view 6 before (left: the full-resolution hillside a darker band, its zigzag edge where the LOD begins) and after (right): the LOD darkens slopes as full resolution's light and occlusion do, measured from both meshers (`lod_slopes_are_as_bright_as_full_resolution`) |
| `polish-4-water-depth-view7-before-after.jpg` | polish 4 | The owner's view 7 before (left: a rift flooded to sea level, as clear 100 blocks down as one) and after (right): water absorbs light with depth, so deep water is deep blue. The disc in the middle is the rift's steep sides, still seen where they lie just under the surface |
| `polish-4-water-depth-coast-before-after.jpg` | polish 4 | A coast from above, before and after: shallows stay clear over the sand, deepening smoothly to blue offshore, near and far alike |
