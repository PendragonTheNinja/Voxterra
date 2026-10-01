# Voxterra — Vision and Ideas

*Started 2026-10-01 from Nathan's notes at the close of M10, with Claude's
comments. A living document: add to it whenever an idea is worth keeping.*

**What this is.** A record of where the game is going, so ideas are written
down once instead of re-thought every few months. **Nothing here is a
decision.** Each idea becomes a milestone spec and, where it changes the
architecture, an ADR before anything is built, and CLAUDE.md's "Intentionally
deferred" list still governs what may be built now. Where an idea conflicts
with a rule in CLAUDE.md, this document says so rather than quietly assuming
the rule will change.

Each section gives **the idea** (Nathan's, in substance) and then **Claude's
view** (an opinion, open to disagreement).

---

## The game, in one paragraph

A genuinely realistic Earth in voxel form — real geology, real climate, real
rivers — with mythology and magic woven through it. Not a plain sandbox:
players start in the Stone Age with nothing and climb, tier by tier, toward
modern technology, with magic and technology each speeding the other along.
Creatures are the real animals of each biome, plus rare mythological ones found
where their legends come from. Combat is closer to Valheim than to Minecraft.
And the world is highly customisable at creation.

---

## 1. World generation

### 1.1 Biomes, and how they scale with world size

**The idea.** Decide the full list of biomes up front, *then* build them and
their distributions, so that they scale with world size. Otherwise someone who
makes a small world could lock themselves out of whole biomes.

**Claude's view.** Agreed on both halves — decide the catalogue first, and
guarantee every biome in every legal world. How to get there:

- **Biomes should be classified from climate, not placed.** Temperature (from
  latitude and altitude) and precipitation (from ocean distance, prevailing
  wind and rain shadow behind mountains) give a Köppen/Whittaker class per
  column. That is pillar 2 doing its job: deserts appear in the rain shadow of
  a range because the range is there.
- **Climate already scales with world size for free.** Latitude loops through
  its full range on every lap of Z whatever the size (ADR-0012), so every
  climate band exists in every world; only its *width* changes. Equator to pole
  is a quarter of the world: 8 192 blocks in the smallest world (~24 minutes'
  sprint), 51 200 at the default, 524 288 at the largest.
- **Landforms must not scale the same way.** ADR-0010 tuned mountains to
  walking time (a range crossed in ~21 minutes), and that should hold in every
  world, or a big world becomes empty and a small one a single mountain. So
  there are two scales: *climate scale* proportional to world size, and
  *landform scale* tied to the player's legs. The catch is biomes that need a
  landform — alpine needs mountains, a rain-shadow desert needs a range to cast
  it — so in the smallest worlds landform scale has to shrink enough that a few
  ranges still fit.
- **Make the guarantee a test.** "Every biome in the catalogue appears in a
  world of every legal size, for many seeds" is a headless test, in the same
  spirit as the per-world coastline calibration (ADR-0012), which already
  stopped land fraction depending on the seed.
- **Pick the catalogue from real-world analogues** (temperate rainforest,
  boreal forest, hot desert, savanna, tundra…), not invented biomes. Creature
  spawns (section 5) then map onto real places and their folklore.

### 1.2 World-creation settings

**The idea.** Eventually, sliders at world creation for the scale of
landmasses, how much of the world is water versus land, the scale of the
terrain itself, and many small things like that. The game should be highly
customisable in the end.

**Claude's view.** The engine is already partly there: world size is per-world
(ADR-0012), and each world calibrates its coastline to a target land fraction —
which is exactly a "land vs water" slider waiting for a UI. Two rules for every
future slider:

- **It is recorded in `world.meta` and covered by the terrain fingerprint**,
  like size and generator version, or a world reopens with different terrain.
- **It is added when the system it controls exists**, not as a speculative
  hook. The settings screen is best built after climate and hydrology, when
  there are meaningful knobs (continent scale, relief, sea fraction, climate
  extremes, river density).

### 1.3 Water — rivers, currents, physics

**The idea.** Something more creative than Minecraft's water, even if it is
complex: real rivers, with currents.

**Claude's view.** This is two systems, and they should be built separately:

1. **Rivers from worldgen (hydrology).** Rain falls, drains downhill, gathers
   into streams and rivers that carve valleys and end in lakes or the sea. A
   river needs to know its upstream, which no single chunk can, so this needs
   a cached region-scale drainage pass (already noted in M10's non-goals). The
   water itself can be **static but carry a flow direction and speed** per
   block, so a river has a current that pushes swimmers, boats and items
   without simulating fluid. Erosion belongs in the same milestone, since
   rivers are what carve valleys.
2. **Dynamic water (simulation).** What happens when a player breaks a dam or
   pours a bucket: finite volume, flowing until level. This is a simulation
   system (the planned `vox-sim` crate), much later, and must never be run
   across a whole ocean. Worldgen water stays settled until disturbed.

Hydrology is also what makes placer gold, floodplain soils and river-valley
settlements possible, so it should come before geology and ores.

### 1.4 Natural slopes and partial blocks

**The idea.** Not everything should be blocky. Naturally generated terrain
could have sloped blocks where the ground steps by one block diagonally, so
hillsides read as slopes and can be walked up. Breaking one still drops the
ordinary full block. Possibly several slope angles, matching the real incline.
Either new slope blocks are generated, or the existing blocks themselves take
a sloped shape. If slopes prove too much, slabs between steps would do — but
slopes would look better and make the game feel more distinctive.

**Claude's view.** Worth doing, and fits pillar 3 — fewer jumps up every
hill — and CLAUDE.md already says slopes and stairs matter. The second version
of the idea (*the block itself takes a shape*) is the right one:

- **Shape is part of the block state.** A block is material × shape (full,
  slab, a few slope and corner variants). The palette makes that cheap: a
  hillside of sloped granite is just more palette entries, and breaking it
  drops the material as a full cube.
- **Worldgen picks the shape from the neighbouring heights**, deterministically,
  so it stays per-chunk independent.
- **Build one shape system, and let slabs be its first shape**, rather than
  building slabs and then slopes. The hard parts are the same for both and
  should be paid once:
  - **Meshing** — the greedy mesher merges full faces only; shaped blocks need
    their own path.
  - **Lighting and culling** — a slope is neither opaque nor transparent. It is
    a third answer to ADR-0011's `opaque` question, which affects face culling,
    AO and light propagation.
  - **Collision** — slopes as walkable ramps, alongside auto-step.
  - **LOD** — the heightfield LOD is already smooth, so distant slopes cost
    nothing extra.
- **Timing:** after biomes settle the surface materials, since every slope
  depends on what the surface is made of.

### 1.5 Atmosphere — distant things turn blue

**The idea.** Far-off things should shift blue the way real mountains do: of
three mountains at increasing distance, the farthest is fainter and more
blue-purple. Fog should do that rather than fade to white — what you are really
seeing is the mountain through the atmosphere, which shifts its colour. Roughly
simulating Rayleigh scattering.

**Claude's view.** Physically right. This is *aerial perspective*: air scatters
short wavelengths most (Rayleigh), so light from a distant mountain loses its
blue-to-red balance on the way to your eye while the air in between adds
scattered sky-blue in front of it. Dark distant terrain turns blue-violet;
bright snow yellows slightly. In a shader it is cheap: per-channel extinction
`exp(-β·d)` with β largest for blue, plus in-scattering toward the sky colour
for the same view direction. M11 criterion 9 already asks for aerial
perspective; it is now noted there that it should be wavelength-dependent. At
32–64 km of view it is the single biggest cue for distance.

---

## 2. Building — chisels

**The idea.** Very far off: chisel tools that reshape blocks in the world.
Several kinds — from cutting out individual voxels of a block, as Vintage Story
does, to chisels that cut specific shapes such as slopes, bevels and cubes, as
Boundless does.

**Claude's view.** Two distinct features with different costs:

- **Shape chisels (Boundless-style)** come almost free once the block-shape
  system of 1.4 exists: the chisel changes a block's shape state. A natural
  early use of that system.
- **Sub-voxel chiselling (Vintage Story-style)** stores a fine grid *inside* a
  block (VS uses 16³), as per-block extra data, with its own meshing,
  collision and lighting. **It conflicts with CLAUDE.md's "No sub-meter voxel
  grid"** as written. That rule protects the *world* grid, and a carved block
  stays one 1 m³ cell holding detail, so it can probably be reconciled — but
  only through an ADR that says so explicitly, not by assuming the rule bends.

---

## 3. Progression — Stone Age to modern technology

**The idea.** A clearer progression system than Minecraft's plain sandbox.
Tiered metals are fine, but beyond that, players should be able to climb
through ever more advanced technology, all the way from the Stone Age with
nothing to modern technology — in the spirit of the GregTech: New Horizons
modpack, but less punishing and less time-consuming.

**Claude's view.** This plays to pillar 2 unusually well: the realistic geology
*is* the progression map. Copper near porphyry intrusions, tin in granite
pegmatites, iron in banded formations, coal in sedimentary basins — each tier
sends the player somewhere new in the world. Principles for keeping it
GTNH-deep but not GTNH-long:

- **Each tier unlocks a new verb, not just bigger numbers** — smelting,
  casting, machining, electricity, automation — so every tier changes how you
  play.
- **Automation arrives early enough to replace repetition**, never as a reward
  for enduring it (pillar 3).
- **Recipes are learned from the world**, not looked up on a wiki, wherever
  possible.

---

## 4. Fantasy and magic alongside realism

**The idea.** A strong mix of fantasy and magic with real-world realism, where
the two help each other: magic speeds technology, and technology builds
infrastructure for magic. Example: tiered grappling hooks — the first one is
available long before you could build the technology to fire and retract it,
because it is partly magic-powered; later tiers improve as both advance.

**Claude's view.** The interlock is what makes this distinctive rather than
two games side by side. A rule worth adopting: **magic should be grounded in
the same world model** — tied to geology (minerals with properties), biome,
time of day or the moon (which the sky already models since M07) — so it
rewards the same exploration and knowledge that technology does.

---

## 5. Creatures — real and mythological

**The idea.** Mobs specific to each biome, following the real world, plus rare
mythological creatures that spawn in the biome their stories come from — the
squonk, Sasquatch and so on — some so rare they are genuinely special to find.

**Claude's view.** This is why the biome catalogue should be real-world
analogues (1.1): folklore then maps cleanly to a biome. The squonk is from the
hemlock forests of Pennsylvania (temperate forest); Sasquatch from the Pacific
Northwest (temperate rainforest). Rarity tiers per biome, and perhaps
conditions (time, weather, depth), make a sighting an event. Entities are a
large system of their own (`vox-sim`, simulation distance, saving), so this
waits on that infrastructure.

---

## 6. Combat

**The idea.** Different from Minecraft and Vintage Story — take much of
Valheim's combat and bring it over in a similar form.

**Claude's view.** What makes Valheim's combat work, as a checklist for the
spec when it comes: a stamina economy shared by attacking, blocking, dodging
and sprinting; blocking with a timed **parry** that staggers; a dodge roll
with invulnerability frames; weapon classes with distinct movesets and damage
types (slash, pierce, blunt) against enemy resistances; enemies that telegraph
their attacks. Combat should also respect the world model — terrain, footing
and slopes (1.4) matter in a fight.

---

## 7. Interface — sooner rather than later

**The idea.** More of the GUI soon: a hotbar; the held block visible at the
bottom right of the screen, as if in hand; an inventory, though that can wait
until survival mode is being built. And a setting to hide the HUD for
screenshots.

**Claude's view.** Agreed this should come soon: it is small, it improves
every play-test (choosing what to place), and screenshots matter for releases
(`docs/releases.md`). Notes for the spec:

- **Hotbar** in the existing egui pass, like the crosshair.
- **Held block** drawn last with its own projection and a cleared depth range,
  so it never clips into walls. It must use `vox_render::perspective` and the
  reversed-Z `DEPTH_*` constants (ADR-0013), like everything else.
- **Hide HUD** on a key (F1 is the convention) and in the ESC menu.
- **Inventory** waits for survival mechanics, as suggested.

---

## 8. Customisability as a principle

**The idea.** The game should be highly customisable in the end.

**Claude's view.** Consistent with the hardcore-mechanics toggles CLAUDE.md
already requires. Practical rule: **every setting is chosen deliberately and
recorded** — world-generation settings in `world.meta` (1.2), gameplay toggles
in a versioned settings file — so a world always reopens the way it was made.

---

## 9. Proposed order

Claude's recommendation for getting the world generator to look realistic and
natural, then on to the game. Each step needs the one before it.

| # | Step | Why here |
|---|---|---|
| M11 | **The Horizon** (spec written), with task 0 (why relighting doubled) and wavelength-dependent atmosphere (1.5) | Long views make every later worldgen step visible from tens of kilometres away |
| — | **HUD basics** (section 7): hotbar, held block, hide HUD | Small; improves every play-test after it. Could slot in before or after Climate |
| — | **Biome catalogue** — a design document, no code | The list must be fixed before climate is built to produce it (1.1) |
| M12 | **Climate & Biomes** — temperature, precipitation, rain shadow, Köppen classes, surface materials, the size-scaling guarantee | Needs mountains (M10) to cast rain shadows |
| — | **Hydrology & erosion** — drainage, rivers with current, lakes above sea level, valleys | Needs precipitation; a river must know its upstream |
| — | **Geology** — strata, caves, ores | Placer gold needs rivers; ores need the rock types |
| — | **Block shapes & natural slopes** (1.4) | Needs final surface materials; touches meshing, lighting, collision once |
| — | **Vegetation** — trees and plants per biome | Needs biomes and surfaces; trees on slopes need shapes |
| — | **World-creation settings** (1.2) | Once the systems with knobs exist |
| — | Then the game: inventory and survival basics; entities (`vox-sim`); creatures (5); combat (6); progression (3); magic (4); dynamic water (1.3); chisels (2) | Gameplay waits for the world it happens in |

Milestone numbers after M12 are left blank deliberately; they are assigned when
each spec is written. The game-era ordering is the loosest part of this table
and should be revisited once the world generator is complete.
