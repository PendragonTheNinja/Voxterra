# Voxterra benchmark

- GPU: llvmpipe (LLVM 20.1.2, 256 bits), 800×600
- World: seed 0x7e22ac0de, 204 × 204 km; start (-19712, 20480)
- Settings: defaults (full-res radius 8, curvature on, air density 1), spectator, noon, time frozen
- Flowing water: on

| Phase | Settle s | fps median | 1% low | Worst ms | Relight ms/s | Mesh ms/s | Dirty peak / end | LOD nodes | GPU MB | Tris M | Loaded |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 6 levels, land, standing | 18.4 | 6 | 4 | 248.9 | 0 | 0 | 0 / 0 | 1216 | 473 | 1.52 | 1427 |
| 6 levels, land, sprint | 1.2 | 5 | 4 | 325.9 | 26 | 24 | 1009 / 725 | 1424 | 494 | 1.28 | 882 |
| 8 levels, land, standing | 20.3 | 5 | 4 | 268.9 | 0 | 0 | 0 / 0 | 1600 | 680 | 1.78 | 1427 |
| 8 levels, land, sprint | 1.4 | 5 | 3 | 311.6 | 21 | 20 | 874 / 571 | 1872 | 734 | 1.50 | 719 |
| 6 levels, ocean, standing | 23.1 | 5 | 4 | 257.0 | 0 | 0 | 0 / 0 | 1216 | 473 | 1.53 | 1475 |
| 6 levels, ocean, sprint | 1.3 | 6 | 4 | 292.7 | 26 | 25 | 1088 / 827 | 1424 | 470 | 1.23 | 973 |
| 8 levels, ocean, standing | 26.4 | 5 | 4 | 280.1 | 0 | 0 | 0 / 0 | 1600 | 679 | 1.77 | 1475 |
| 8 levels, ocean, sprint | 1.4 | 5 | 3 | 335.3 | 20 | 20 | 771 / 647 | 1872 | 709 | 1.40 | 739 |
| 8 levels, rivers, standing | 23.6 | 5 | 4 | 273.2 | 0 | 0 | 0 / 0 | 1834 | 822 | 1.74 | 1489 |
| 8 levels, rivers, sprint | 1.4 | 4 | 3 | 367.3 | 20 | 17 | 818 / 645 | 2039 | 898 | 1.67 | 788 |
| 8 levels, waterfall, standing | 60.1 (timed out) | 3 | 2 | 409.5 | 0 | 8 | 476 / 0 | 1848 | 976 | 3.69 | 1542 |

| Phase | Meshed /s | Mesh µs/chunk (CPU) | Encode ms median / p99 | Draws | Tris submitted M | Tris drawn M | Zero-area % | Dirty end: ready / relight / neighbours |
|---|---|---|---|---|---|---|---|---|
| 6 levels, land, standing | 0 | 0 | 174.64 / 228.24 | 620 | 1.52 | 1.52 | 0 | 0 / 0 / 0 |
| 6 levels, land, sprint | 148 | 197 | 156.11 / 238.22 | 524 | 1.28 | 1.28 | 0 | 73 / 625 / 27 |
| 8 levels, land, standing | 0 | 0 | 200.04 / 265.68 | 691 | 1.78 | 1.78 | 0 | 0 / 0 / 0 |
| 8 levels, land, sprint | 149 | 149 | 174.37 / 270.13 | 570 | 1.50 | 1.50 | 0 | 125 / 446 / 0 |
| 6 levels, ocean, standing | 0 | 0 | 183.73 / 244.94 | 678 | 1.53 | 1.53 | 0 | 0 / 0 / 0 |
| 6 levels, ocean, sprint | 185 | 157 | 148.42 / 233.46 | 531 | 1.23 | 1.23 | 0 | 33 / 794 / 0 |
| 8 levels, ocean, standing | 0 | 0 | 213.62 / 276.73 | 741 | 1.77 | 1.77 | 0 | 0 / 0 / 0 |
| 8 levels, ocean, sprint | 174 | 121 | 181.53 / 281.67 | 574 | 1.40 | 1.40 | 0 | 254 / 393 / 0 |
| 8 levels, rivers, standing | 0 | 0 | 214.93 / 270.10 | 648 | 1.74 | 1.74 | 0 | 0 / 0 / 0 |
| 8 levels, rivers, sprint | 119 | 161 | 215.51 / 310.39 | 511 | 1.67 | 1.67 | 0 | 42 / 602 / 1 |
| 8 levels, waterfall, standing | 23 | 287 | 356.38 / 406.40 | 914 | 3.69 | 3.69 | 0 | 0 / 0 / 0 |
