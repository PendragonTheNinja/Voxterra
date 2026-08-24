# Releases and the version archive

**Goal:** keep every milestone of Voxterra playable and browsable forever, the
way Minecraft's old versions are — so the engine's growth can be seen, not just
described.

Nothing has been lost. Every milestone is already a commit; this document turns
that history into an archive people can actually use.

---

## 1. Tag every milestone (do this once, retroactively)

Tags turn commits into named versions with a browsable page on GitHub. Every
milestone has a clean completion commit. Run once, from the repo root:

```powershell
git tag -a v0.0.0-m00 cab6a90 -m "M00: spinning chunk — camera, pipeline, test scene"
git tag -a v0.1.0-m01 50ea78d -m "M01: real chunk system — palette storage, greedy meshing, frustum culling"
git tag -a v0.2.0-m02 0a535ea -m "M02: infinite world — streaming, persistence, floating origin"
git tag -a v0.3.0-m03 2dc5fdb -m "M03: blocks, interaction, textures"
git tag -a v0.4.0-m04 e3c0fb1 -m "M04: block light — torches, parallel relighting"
git tag -a v0.5.0-m05 92af972 -m "M05: skylight — heightmap propagation, survival movement"
git tag -a v0.6.0-m06 80dde75 -m "M06: smooth lighting + ambient occlusion"
git tag -a v0.7.0-m07 78b8e37 -m "M07: day/night cycle — sun, phase-lit moon, starfield"
git tag -a v0.8.0-m08 ad19b20 -m "M08: single-level LOD — a real horizon"
git push --tags
```

Going forward, tag at the end of each milestone, right after the retrospective
commit:

```powershell
git tag -a v0.9.0-m09 -m "M09: LOD levels & streaming quality"
git push --tags
```

**Scheme:** `v0.<milestone>.<patch>-m<NN>`. Minor version tracks the milestone;
bump the patch for a fix to an already-tagged release. Version 1.0 is a
deliberate later decision, not an automatic consequence of reaching M10.

Anyone can now browse or download any version's source from the repo's Tags
page, and `git checkout v0.7.0-m07 && cargo run -p vox-app --release` plays that
version.

## 2. Publish built executables (what makes it a *playable* archive)

Source tags serve developers. Minecraft's archive is compelling because old
versions are **downloadable and runnable** by people who won't compile anything.
For each tag, attach a build to a GitHub Release:

```powershell
git checkout v0.8.0-m08
cargo build --release
# zip target\release\vox-app.exe together with any runtime assets
```

Then, on GitHub: Releases → Draft a new release → pick the tag → attach the zip
→ paste the milestone's retrospective summary as the notes.

Two things worth knowing:

- **Archive the binary at release time.** Rebuilding an old tag years later may
  not produce an identical program: toolchains and dependencies drift.
  `Cargo.lock` is committed, which helps a great deal, but the .exe built the
  day the milestone shipped is the only fully faithful artifact.
- **Builds are currently Windows-only.** Cross-platform releases are a future
  decision; until then, say so in the release notes rather than letting people
  guess.

## 3. Capture media per milestone (the part that cannot be done later)

This is the only item that is genuinely lost if skipped, because it needs the
build running *at that moment*. Tags preserve code; they do not preserve what
the world looked like.

At each milestone, save into `docs/gallery/mNN/`:

- 3–5 screenshots of what the milestone added (the first sunset, the first
  horizon, the first torch-lit cave).
- A short clip if the feature moves — a day/night cycle, LOD resolving as you
  fly toward it.
- The telemetry line from a representative run, so performance history is
  visible alongside the visuals.

Reference them from the milestone's retrospective. In a year this is what makes
the archive worth browsing.

## 4. Save-format compatibility (the trap)

Old builds and new builds do **not** reliably share a `world/` folder. This has
already bitten once: the M08 terrain change made previously generated chunks
seam badly against new ones, and the fix was to wipe `world/`.

Until the save format carries a version, each release should either ship its
own world folder or state plainly in the notes: **start a fresh world**. When
persistence is next revisited, writing a worldgen/format version into the save
and refusing (or migrating) mismatched worlds is the durable fix — and it is
precisely what makes an old build still work off its own saved world years
later.

## Release checklist

1. Milestone retrospective written, `CLAUDE.md` status updated.
2. Full gauntlet green: `cargo build`, `cargo clippy --workspace --all-targets
   -- -D warnings`, `cargo fmt`, `cargo test`, and a real play session.
3. Commit, push.
4. Tag (`git tag -a … -m …`), `git push --tags`.
5. `cargo build --release`; zip the executable with its assets.
6. GitHub Release: attach the zip, notes from the retrospective, state the
   platform and the fresh-world requirement.
7. Screenshots/clip into `docs/gallery/mNN/`.
