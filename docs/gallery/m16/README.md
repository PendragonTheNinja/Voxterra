# M16 gallery — Tectonics & Bedrock

Seed `0x7e22ac0de`, default size (204.8 km) unless a row says otherwise.
Every image made during the milestone is kept here, with the commit it
shows.

| Image | Commit | What |
|---|---|---|
| `tectonics-7e22ac0de.jpg` | task 1 | The tectonic framework beside the terrain it does not yet place (`cargo test -p vox-worldgen --release tectonics_map -- --ignored`, one pixel per 256 blocks). **Left, the framework:** each plate its own colour — pale where it carries continent, palest on cratons, dark where it is ocean floor; a white line from each plate's centre is its drift; a ringed white dot with a tail is a hotspot and its track. Boundaries by kind: **red** collision, **orange** continental arc, **yellow** island arc, **cyan** spreading ridge, **magenta** rift, **black** transform, **cream** passive margin, **brown** sutures, thin **grey** seams. **Middle, what task 2 will read:** continental crust tan and ocean blue (`continentalness`, clamped 3 500 blocks either side of the coast) with the belt envelope in white — arcs along the active margins, old welds wrapping the cratons, island arcs at sea, nothing on ridges, transforms or passive coasts. **Right:** today's terrain (generator version 8), placed by noise; the two do not agree yet, and making them agree is task 2. The map wraps on all four edges |
| `tectonics-7e22ac0de-smallest.jpg` | task 1 | The same for the smallest legal world (32 768 blocks, one pixel per 82): nine terranes on three plates |
