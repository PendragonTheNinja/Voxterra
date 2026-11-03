# Water — how it behaves (design, for the owner's decision)

*Written 2026-11-01 for M14 task 0. A design document, not a decision: the
owner decides, and the decision becomes an ADR. The simulation itself is a
later milestone (`vox-sim`); M14 only has to store worldgen water so that
the chosen model stays possible.*

**The owner's leaning (2026-11-01):** a hybrid — simulating the whole water
cycle as finite volume is "an insane amount of simulation for something that
technically won't matter", and would make the game keep up with all water at
all times; realistic water inside *localised disturbances* would be cool, if
it performs; stepped levels rather than smooth gradients are fine in a voxel
game. The owner asked what is best for the long term, and whether there is
real, fun gameplay in finite water.

## Why Minecraft-style games keep water simple

The reasons are real, and worth taking one by one, because each has an
answer in this engine that a 2009 engine did not have:

1. **Cost and reach.** A water simulation must run wherever water can move.
   In an open world that is everywhere loaded — and water at the edge of
   the loaded area must freeze, then resume, without leaking or duplicating.
   Minecraft's infinite sources make that trivial: a source never runs out,
   so nothing needs conserving across a boundary.
2. **The ocean drain.** With finite water, a hole in the seabed to a cave
   tries to drain an ocean through a world that is mostly unloaded. Either
   the sea level drops (absurd), or water is created (then it is not finite).
3. **Griefing and landscape damage.** A lake drained by a careless tunnel
   floods a valley forever; a player's base is destroyed by one bucket.
4. **Small-water tedium.** Finite water makes ordinary tasks harder — a
   farm's water runs out, an infinite pool needs a pump — and in a pure
   sandbox that reads as friction, not fun.
5. **Rendering partial water** means partial-height surfaces and constant
   remeshing as levels change.

Minecraft chose sources because they make all five disappear. **The reason
is good — for a game whose water is only a building material.** It is not
good enough for this one, whose pillars are a realistic Earth (rivers, lakes,
water tables) and *interesting decisions* (CLAUDE.md pillar 3: "shore the
tunnel or risk a cave-in?"). Water is the second great hazard of mining after
collapse; a game about real geology without flooding mines is missing half of
what mining was.

## The three models

| | Minecraft-like | Finite everywhere | **Hybrid (recommended)** |
|---|---|---|---|
| What water is | sources (infinite) and flowing levels 0–7 | an amount per cell, conserved, moved by gravity and pressure | worldgen water settled and asleep; finite water where disturbed; reservoirs refill from the drainage network |
| A river | a staircase of sources | must be fed by rain upstream, continuously | settled blocks; its flow is in the network |
| Break a riverbank | water spreads 7 blocks, endless | the river drains into your hole | water floods in, endlessly — the river is a reservoir |
| Drain a pond | impossible (sources) | yes | yes — small lakes are finite |
| Hole in the seabed | water flows, endless | the ocean drains | floods, endlessly — the sea is a reservoir |
| Cost | tiny | enormous, always | proportional to disturbances, zero when settled |
| Realism | low | highest | high where the player can see it |

The hybrid is not a compromise between the other two so much as the
physically right reading of them: **seas, rivers and large lakes really are
effectively infinite reservoirs at a player's scale**, and a puddle, a pond,
a flooded mine shaft really are finite. Earth's water cycle is the drainage
network's job (stage 1, M14); the simulation only ever handles what the
player has disturbed.

## Recommendation: the hybrid, in three layers

### Layer 1 — settled water (M14, worldgen)

All generated water is full, settled blocks; river surfaces step down their
long profile in whole blocks. The drainage network records, for every river
and lake, its level, flow direction, speed and discharge, and whether it is a
**reservoir** (sea, river above a discharge threshold, lake above a size)
or **finite** (ponds, tarns, puddles of the wetlands). Nothing simulates.

### Layer 2 — local finite water (its own milestone, `vox-sim`)

- **Wakes on disturbance only**: breaking or placing next to water, or
  emptying a bucket, marks cells active. A settled world costs nothing.
- **Cells hold eighths of a block** (0–8): stepped, as the owner said is
  fine, cheap to store (a sparse per-chunk layer for disturbed water only;
  settled water stays a plain `WATER` block in the palette), and renderable
  with the partial-height shapes the block-shapes milestone builds.
- **Rules**: fall first, then spread to equalise with lower neighbours;
  simple pressure (a column pushes up the other side of a U-bend) as a
  later refinement; puddles of one eighth dry out over time so thin films
  do not cover the world.
- **Reservoir boundary**: a cell touching settled reservoir water draws from
  it without limit — the river keeps pouring into your trench, the sea into
  your canal, the way they do. A finite lake drains and stays drained.
- **Settles and sleeps** when nothing changed for a while; active regions
  freeze at the edge of the loaded area and resume when it loads.
- **Budget**: a fixed number of cell updates per tick, nearest first, like
  every other queue in this engine. Water is not opaque, so a water change
  needs **no relight** — only a remesh, which M12 made cheap (~0.2 ms a
  chunk).
- **Permanent changes** (a drained lake, a new canal) are saved with the
  chunk, and fed to the LOD the way edits are (`EditedColumns`).

### Layer 3 — water as gameplay (with progression)

Where finite water earns its cost — each an *interesting decision*:

- **Flooding mines.** Dig below the water table near a river or lake and
  water seeps in; break into an aquifer and it floods. Shore it, pump it, or
  route it away — mining's second great hazard beside collapse, and pumps
  are why the steam engine was invented.
- **Draining and damming.** Drain a pond to reach clay or placer gold on its
  bed; dam a stream to make a millpond; breach a dam on purpose — or by
  accident.
- **Irrigation.** Crops need water; channels from a river turn a dry valley
  green, and the choice of where to cut them is real planning.
- **Water power.** A water wheel turns with the river's recorded flow
  (stage 1): faster on a steep stream, steadier on a big river — the first
  machine power before steam.
- **Wells and the water table.** Dig down until you hit water: shallow by a
  river, deep in a desert, absent on a mountain — geology and climate made
  tangible.
- **Floods** (with seasons, much later): rivers rising after rain.

Kept out on purpose: **anything that lets one bucket wreck a world.** A
bucket is a finite volume; reservoirs cannot be emptied; a griefer can flood
a hollow, not a valley. Hardcore realism (aquifers that drain wells,
evaporation everywhere) belongs behind the hardcore toggles CLAUDE.md
already promises for cave-ins.

## What M14 must do now

1. Store worldgen water as plain, full `WATER` blocks — no water state in
   the palette that a later model would have to undo.
2. Record in the cached drainage network, per river and lake: water level,
   flow direction and speed, discharge, and **reservoir or finite**.
3. Keep the water-level fill rule (water above the ground up to the local
   water level) a single function, as `materials::block_in` is now, so the
   simulation can ask what "settled" means for any column.

## For the owner to decide

- The hybrid, in principle — or another model.
- What counts as a reservoir: the sea, rivers above some discharge, lakes
  above some size; everything else finite.
- When layer 2 is built: after block shapes (partial-height water needs
  them), before or with mining hazards.
