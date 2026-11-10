# Bench results

`cargo run -p vox-app -- --bench` writes its report to `bench/`, which git
ignores (it also holds the benchmark's own world). A report worth keeping —
a milestone's baseline, a before/after pair — is copied here and committed,
with a line below saying what it is.

The benchmark runs in its own world, `bench/world-v<generator version>`,
created from the default seed on first use. It never touches `world/`.

| File | What it shows | Commit |
|---|---|---|
| `2026-11-audit-cloud-before.md` | The audit's "before": `main` at the M15 catalogue merge. **Cloud container, software GPU (llvmpipe), 4 cores, ~5 fps** — the frame is rasteriser-bound, so only the streaming columns (settle, relight, mesh, worst frame) mean anything, and only against its pair | `cbef977` |
| `2026-11-audit-cloud-after.md` | The same run after the audit's engine changes (before the relight CPU columns were added to the report) | `3b9d347` |

The M14 re-bench at generator version 8, on the owner's GPU, is M15's
baseline and is still to come.
