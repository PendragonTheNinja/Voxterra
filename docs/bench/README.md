# Bench results

`cargo run -p vox-app -- --bench` writes its report to `bench/`, which git
ignores (it also holds the benchmark's own world). A report worth keeping —
a milestone's baseline, a before/after pair — is copied here and committed,
with a line below saying what it is.

The benchmark runs in its own world, `bench/world-v<generator version>`,
created from the default seed on first use. It never touches `world/`.

| File | What it shows | Commit |
|---|---|---|
| *(none yet — the M14 re-bench at generator version 8 is M15's baseline)* | | |
