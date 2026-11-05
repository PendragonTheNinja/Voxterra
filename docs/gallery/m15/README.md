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
