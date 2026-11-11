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

| `bench-1794373242.md` | **M15's baseline** (2026-11-11): generator version 8 on the audited build, with the tectonic framework built but unread. The owner's RTX 4060, 800×600, dev build. Against `bench-1793567649.md` below: sprint 786–866 fps (was 654–730), 1% low 72–77 (58–66), relight 76–95 ms/s (157–196), backlog at the end of a sprint 0 (219–246; one phase 119), mesh 180–194 µs per chunk (202–213); worst sprint frame unchanged at 31–46 ms | `bed24e6` |
| `bench-1793567649.md` | The last M14 bench before the audit (2026-11-01, generator version 4): the "before" for the row above | — |
| other `bench-*.md` | The owner's raw reports from M12–M14, named by Unix time; what each showed is in that milestone's spec | — |

Criterion 9 of M15 ("within ~10% of M14's version-8 re-bench") compares
against `bench-1794373242.md`.
