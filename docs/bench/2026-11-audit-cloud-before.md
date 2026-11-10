# Voxterra benchmark

- GPU: llvmpipe (LLVM 20.1.2, 256 bits), 800×600
- World: seed 0x7e22ac0de, 204 × 204 km; start (-19712, 20480)
- Settings: defaults (full-res radius 8, curvature on, air density 1), spectator, noon, time frozen
- Flowing water: on

| Phase | Settle s | fps median | 1% low | Worst ms | Relight ms/s | Mesh ms/s | Dirty peak / end | LOD nodes | GPU MB | Tris M | Loaded |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 6 levels, land, standing | 24.7 | 5 | 3 | 310.2 | 0 | 0 | 0 / 0 | 1216 | 473 | 1.52 | 1427 |
| 6 levels, land, sprint | 1.2 | 5 | 3 | 338.2 | 63 | 20 | 854 / 576 | 1424 | 493 | 1.27 | 713 |
| 8 levels, land, standing | 28.2 | 5 | 4 | 273.1 | 0 | 0 | 0 / 0 | 1600 | 680 | 1.78 | 1427 |
| 8 levels, land, sprint | 1.2 | 4 | 3 | 405.7 | 50 | 19 | 756 / 442 | 2128 | 825 | 1.50 | 526 |
| 6 levels, ocean, standing | 27.4 | 5 | 4 | 270.4 | 0 | 0 | 0 / 0 | 1216 | 473 | 1.53 | 1475 |
| 6 levels, ocean, sprint | 1.3 | 5 | 3 | 372.0 | 53 | 21 | 763 / 451 | 1424 | 470 | 1.23 | 591 |
| 8 levels, ocean, standing | 32.0 | 5 | 4 | 273.5 | 0 | 0 | 0 / 0 | 1600 | 679 | 1.77 | 1475 |
| 8 levels, ocean, sprint | 1.2 | 4 | 3 | 374.1 | 50 | 20 | 788 / 521 | 1872 | 709 | 1.43 | 643 |
| 8 levels, rivers, standing | 29.2 | 5 | 4 | 280.7 | 0 | 0 | 0 / 0 | 1834 | 822 | 1.74 | 1489 |
| 8 levels, rivers, sprint | 1.3 | 4 | 2 | 477.4 | 52 | 8 | 830 / 624 | 2044 | 899 | 1.67 | 723 |
| 8 levels, waterfall, standing | 60.1 (timed out) | 3 | 2 | 513.4 | 1 | 11 | 546 / 0 | 1845 | 975 | 3.67 | 1542 |

| Phase | Meshed /s | Mesh µs/chunk (CPU) | Encode ms median / p99 | Draws | Tris submitted M | Tris drawn M | Zero-area % | Dirty end: ready / relight / neighbours |
|---|---|---|---|---|---|---|---|---|
| 6 levels, land, standing | 0 | 0 | 176.64 / 279.20 | 620 | 1.52 | 1.52 | 0 | 0 / 0 / 0 |
| 6 levels, land, sprint | 126 | 205 | 156.05 / 249.14 | 522 | 1.27 | 1.27 | 0 | 0 / 552 / 24 |
| 8 levels, land, standing | 0 | 0 | 213.28 / 269.55 | 691 | 1.78 | 1.78 | 0 | 0 / 0 / 0 |
| 8 levels, land, sprint | 125 | 185 | 181.50 / 289.89 | 569 | 1.50 | 1.50 | 0 | 23 / 402 / 17 |
| 6 levels, ocean, standing | 0 | 0 | 190.04 / 236.07 | 678 | 1.53 | 1.53 | 0 | 0 / 0 / 0 |
| 6 levels, ocean, sprint | 147 | 182 | 165.17 / 264.91 | 527 | 1.23 | 1.23 | 0 | 3 / 423 / 25 |
| 8 levels, ocean, standing | 0 | 0 | 210.07 / 270.27 | 741 | 1.77 | 1.77 | 0 | 0 / 0 / 0 |
| 8 levels, ocean, sprint | 144 | 176 | 176.03 / 279.90 | 571 | 1.43 | 1.43 | 0 | 9 / 511 / 1 |
| 8 levels, rivers, standing | 0 | 0 | 205.48 / 278.36 | 648 | 1.74 | 1.74 | 0 | 0 / 0 / 0 |
| 8 levels, rivers, sprint | 72 | 122 | 211.87 / 360.69 | 507 | 1.67 | 1.67 | 0 | 0 / 624 / 0 |
| 8 levels, waterfall, standing | 27 | 319 | 358.69 / 504.30 | 909 | 3.67 | 3.67 | 0 | 0 / 0 / 0 |
