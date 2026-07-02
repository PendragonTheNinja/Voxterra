//! Block-light propagation (Milestone 04 task 2, ADR-0004).
//!
//! A BFS flood-fill: light-emitting voxels seed their emission level, and
//! light spreads to adjacent non-solid voxels at one level less, stopping at
//! 0 and at solid blocks. Overlapping sources take the brightest value, not
//! the sum.
//!
//! The core ([`propagate_block_light`]) is written against an abstract
//! [`LightVolume`] — a rectangular region exposing per-cell solidity and
//! emission plus get/set light — so it is independent of `Chunk`, the
//! registry, and the renderer, and is exhaustively unit-testable on tiny
//! hand-built grids. A chunk adapter (task 4) implements `LightVolume` over a
//! chunk-plus-neighbor-borders view.
//!
//! ## Cross-chunk spread
//!
//! Like meshing, lighting is computed per chunk with neighbors as read-only
//! boundary context: a chunk relights using its neighbors' border light as
//! fixed inputs (light flows *in* from bright neighbors). When a chunk's
//! border light changes, the dirty-set marks the neighbor for relight, which
//! then sees the updated border. This avoids holding several chunks mutably
//! at once while still converging to globally-correct light.

/// Maximum light level (4-bit, ADR-0004).
pub const MAX_LIGHT: u8 = 15;

use crate::chunk::Chunk;
use crate::coords::{CHUNK_SIZE, LocalPos};
use crate::registry::BlockRegistry;

/// A rectangular volume the propagator can read solidity/emission from and
/// read/write light into. Coordinates are volume-local `(x, y, z)`, each in
/// `0..dim(axis)`. Implementors decide how out-of-this-chunk borders are
/// represented (e.g. an extra ring of cells carrying neighbor light).
pub trait LightVolume {
    /// Dimensions `[x, y, z]` of the volume in cells.
    fn dims(&self) -> [usize; 3];
    /// Whether the cell blocks light (light does not enter solid cells).
    fn is_solid(&self, x: usize, y: usize, z: usize) -> bool;
    /// Light this cell emits on its own (0 for non-emitters).
    fn emission(&self, x: usize, y: usize, z: usize) -> u8;
    /// Current block-light at the cell.
    fn get_light(&self, x: usize, y: usize, z: usize) -> u8;
    /// Set block-light at the cell.
    fn set_light(&mut self, x: usize, y: usize, z: usize, level: u8);
    /// Whether light may propagate INTO this cell during a flood (default:
    /// yes). A volume whose outer shell stands in for *neighbor* light — the
    /// padded chunk's border ring — must answer `false` for shell cells: they
    /// are seeds that push light inward but never receive or relay it. If the
    /// shell conducts, it fabricates paths along the chunk boundary through
    /// space that actually belongs to the neighbor (and may be solid there):
    /// e.g. daylight seeded high on a side plane riding the ring straight
    /// down at full strength, past the surface, into sealed caves.
    fn accepts_spread(&self, _x: usize, _y: usize, _z: usize) -> bool {
        true
    }
}

/// Flood-fill block light through `vol` from scratch: clears non-emitter
/// light to 0, seeds emitters, and BFS-propagates. Cells that are solid hold
/// their emission (so a glowing solid block still reads as lit) but do not
/// propagate beyond their own cell unless they emit.
///
/// Returns the number of cells that ended up lit (>0), useful for telemetry
/// and for deciding whether a chunk needs light storage at all.
pub fn propagate_block_light(vol: &mut impl LightVolume) -> usize {
    let [dx, dy, dz] = vol.dims();

    // Seed: each cell starts at its own emission. (Clearing first guarantees
    // a from-scratch recompute, matching recompute-on-load semantics.)
    let mut queue: Vec<(usize, usize, usize, u8)> = Vec::new();
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let e = vol.emission(x, y, z);
                vol.set_light(x, y, z, e);
                if e > 1 {
                    queue.push((x, y, z, e));
                }
            }
        }
    }

    // BFS. Each dequeued cell pushes light-1 into non-solid neighbors that
    // are currently dimmer, raising them and re-queueing.
    let mut head = 0;
    while head < queue.len() {
        let (x, y, z, level) = queue[head];
        head += 1;

        // A cell may have been raised brighter after being queued; use the
        // current value so we propagate the strongest.
        let level = vol.get_light(x, y, z).max(level);
        if level <= 1 {
            continue;
        }
        let spread = level - 1;

        for (nx, ny, nz) in neighbors(x, y, z, dx, dy, dz) {
            if vol.is_solid(nx, ny, nz) || !vol.accepts_spread(nx, ny, nz) {
                continue;
            }
            if vol.get_light(nx, ny, nz) < spread {
                vol.set_light(nx, ny, nz, spread);
                if spread > 1 {
                    queue.push((nx, ny, nz, spread));
                }
            }
        }
    }

    // Count lit cells.
    let mut lit = 0;
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                if vol.get_light(x, y, z) > 0 {
                    lit += 1;
                }
            }
        }
    }
    lit
}

/// Remove block light, two-phase (ADR-0004), after a light-affecting change
/// at `(sx, sy, sz)` — e.g. a light source was broken, or a solid block was
/// placed where light used to be. Call this with the volume's `emission`/
/// `is_solid` already reflecting the change; it repairs the light field
/// without a full recompute.
///
/// Phase 1 (un-light): from the changed cell, clear cells whose light could
/// only have come from here (their level is strictly less than the cell
/// we're clearing), collecting as re-light seeds any neighbor that is *as
/// bright or brighter* than expected — those are sustained by other sources.
/// Phase 2 (re-light): BFS-propagate outward from those seeds (and from the
/// changed cell's own emission, if it still emits) so surviving light flows
/// back in correctly.
///
/// This is bounded to the affected region, not the whole volume.
pub fn remove_block_light(vol: &mut impl LightVolume, sx: usize, sy: usize, sz: usize) {
    let [dx, dy, dz] = vol.dims();

    // The light level we're removing from the origin cell.
    let start_level = vol.get_light(sx, sy, sz);
    // The cell may still emit (e.g. a solid block placed atop an emitter, or
    // the change didn't actually remove emission); its own emission survives.
    let start_emit = vol.emission(sx, sy, sz);

    if start_level == 0 {
        // Nothing was lit here; only a (re)propagation from emission matters.
        if start_emit > 0 {
            vol.set_light(sx, sy, sz, start_emit);
            let mut q = vec![(sx, sy, sz)];
            bfs_spread(vol, &mut q);
        }
        return;
    }

    // Phase 1: BFS clearing cells fed only by the removed light.
    // Queue carries (cell, level_it_had_before_clearing).
    let mut unlight: Vec<(usize, usize, usize, u8)> = vec![(sx, sy, sz, start_level)];
    // Seed cells that remain lit by other sources → re-propagate from them.
    let mut relight: Vec<(usize, usize, usize)> = Vec::new();

    // Clear the origin first (unless it still emits — handled in phase 2).
    vol.set_light(sx, sy, sz, 0);

    let mut head = 0;
    while head < unlight.len() {
        let (x, y, z, level) = unlight[head];
        head += 1;

        for (nx, ny, nz) in neighbors(x, y, z, dx, dy, dz) {
            let nl = vol.get_light(nx, ny, nz);
            if nl == 0 {
                continue;
            }
            if nl < level {
                // This neighbor's light came (only) from us: clear it and
                // continue un-lighting from it. Its emission, if any, will be
                // restored in phase 2.
                vol.set_light(nx, ny, nz, 0);
                unlight.push((nx, ny, nz, nl));
            } else {
                // nl >= level: sustained by another source. Re-light seed.
                relight.push((nx, ny, nz));
            }
        }
    }

    // Phase 2: re-propagate from surviving borders and any cell that still
    // emits (origin, or emitters we cleared in phase 1).
    let mut q: Vec<(usize, usize, usize)> = Vec::new();
    for (x, y, z) in relight {
        // Only seed cells that currently still hold light.
        if vol.get_light(x, y, z) > 0 {
            q.push((x, y, z));
        }
    }
    // Restore emission for any cleared emitter (including the origin) so it
    // re-seeds the flood.
    let cleared: Vec<(usize, usize, usize)> =
        unlight.iter().map(|&(x, y, z, _)| (x, y, z)).collect();
    for &(x, y, z) in &cleared {
        let e = vol.emission(x, y, z);
        if e > vol.get_light(x, y, z) {
            vol.set_light(x, y, z, e);
            q.push((x, y, z));
        }
    }
    bfs_spread(vol, &mut q);
}

/// Shared BFS spread step used by add/remove: from each seed, push light-1
/// into dimmer non-solid neighbors. Seeds must already hold their light.
fn bfs_spread(vol: &mut impl LightVolume, queue: &mut Vec<(usize, usize, usize)>) {
    let [dx, dy, dz] = vol.dims();
    let mut head = 0;
    while head < queue.len() {
        let (x, y, z) = queue[head];
        head += 1;
        let level = vol.get_light(x, y, z);
        if level <= 1 {
            continue;
        }
        let spread = level - 1;
        for (nx, ny, nz) in neighbors(x, y, z, dx, dy, dz) {
            if vol.is_solid(nx, ny, nz) || !vol.accepts_spread(nx, ny, nz) {
                continue;
            }
            if vol.get_light(nx, ny, nz) < spread {
                vol.set_light(nx, ny, nz, spread);
                queue.push((nx, ny, nz));
            }
        }
    }
}

/// The in-bounds 6-neighbors of a cell.
fn neighbors(
    x: usize,
    y: usize,
    z: usize,
    dx: usize,
    dy: usize,
    dz: usize,
) -> impl Iterator<Item = (usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::with_capacity(6);
    if x + 1 < dx {
        out.push((x + 1, y, z));
    }
    if x > 0 {
        out.push((x - 1, y, z));
    }
    if y + 1 < dy {
        out.push((x, y + 1, z));
    }
    if y > 0 {
        out.push((x, y - 1, z));
    }
    if z + 1 < dz {
        out.push((x, y, z + 1));
    }
    if z > 0 {
        out.push((x, y, z - 1));
    }
    out.into_iter()
}

/// Propagate **sky light** through `vol` from scratch (Milestone 05,
/// ADR-0005). Skylight enters from the top boundary (the open sky, or a
/// neighbor above) and floods down and sideways. Two rules distinguish it
/// from block light:
///
/// 1. **No attenuation straight down at full strength.** A downward step into
///    a non-solid cell keeps the level unchanged *iff* the level is
///    [`MAX_LIGHT`] (direct sunlight). A vertical shaft open to the sky stays
///    15 all the way to the floor. (Restricting no-attenuation to level 15 is
///    the classic behavior; it avoids non-physical pillars of dimmed diffuse
///    light falling forever.)
/// 2. **Open-top boundary = daylight.** `top_sky[x + z*dx]` is the skylight
///    arriving from directly above each column — 15 for open sky, or a
///    neighbor-above's value. Where it is 15 the top non-solid cell is 15
///    (down rule across the boundary); otherwise it enters one dimmer.
///
/// All other steps (horizontal, upward) dim by one, like block light. Solid
/// cells block skylight. This operates on whichever channel the
/// [`LightVolume`] exposes via get/set_light (the chunk adapter points it at
/// the sky nibble); it clears that channel first (from-scratch recompute).
pub fn propagate_sky_light(vol: &mut impl LightVolume, top_sky: &[u8]) {
    let [dx, dy, dz] = vol.dims();
    debug_assert_eq!(top_sky.len(), dx * dz, "top_sky must be per-column (dx*dz)");

    // Clear this channel first.
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                vol.set_light(x, y, z, 0);
            }
        }
    }

    // Seed the top layer from the incoming sky per column.
    let mut queue: Vec<(usize, usize, usize)> = Vec::new();
    let top_y = dy - 1;
    for z in 0..dz {
        for x in 0..dx {
            if vol.is_solid(x, top_y, z) {
                continue; // a roof at the very top blocks this column's sky
            }
            let incoming = top_sky[x + z * dx];
            // Entering downward across the top boundary obeys the down rule.
            let level = if incoming == MAX_LIGHT {
                MAX_LIGHT
            } else {
                incoming.saturating_sub(1)
            };
            if level > 0 {
                vol.set_light(x, top_y, z, level);
                queue.push((x, top_y, z));
            }
        }
    }

    // BFS flood with the directional rules.
    let mut head = 0;
    while head < queue.len() {
        let (x, y, z) = queue[head];
        head += 1;
        let level = vol.get_light(x, y, z);
        if level <= 1 {
            continue;
        }
        for (nx, ny, nz) in neighbors(x, y, z, dx, dy, dz) {
            if vol.is_solid(nx, ny, nz) || !vol.accepts_spread(nx, ny, nz) {
                continue;
            }
            // Straight down means the neighbor is directly below (ny = y-1).
            let going_down = nz == z && nx == x && ny + 1 == y;
            let spread = if going_down && level == MAX_LIGHT {
                MAX_LIGHT
            } else {
                level - 1
            };
            if vol.get_light(nx, ny, nz) < spread {
                vol.set_light(nx, ny, nz, spread);
                queue.push((nx, ny, nz));
            }
        }
    }
}

// ===========================================================================
// Chunk adapter (Milestone 04 task 4)
// ===========================================================================

/// Light planes from a chunk's six face-neighbors, used as boundary input
/// when relighting a chunk. `planes[face]` is the 32×32 light of the
/// neighbor cells touching this chunk on that face (the neighbor's opposite
/// face plane), or `None` if that neighbor isn't loaded (treated as dark).
///
/// Face order matches the registry/mesher: 0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z.
/// In-plane index is `u + v * CHUNK_SIZE`, where (u, v) are the two axes
/// perpendicular to the face, in ascending axis order (X faces: (y, z);
/// Y faces: (x, z); Z faces: (x, y)).
pub type NeighborLight = [Option<Vec<u8>>; 6];

// ---------------------------------------------------------------------------
// Heightmap / sky occlusion (Milestone 05, ADR-0005)
// ---------------------------------------------------------------------------
//
// Skylight needs to know, per (x, z) column, whether the sky is occluded
// above a given cell. In a streaming cubic world the full column is never all
// loaded, so we derive the top-boundary per chunk during relight: a chunk's
// skylight comes in from the cells directly above it (its +Y neighbor), and
// where there is no occluder above, that boundary is full daylight (15).
//
// These headless helpers compute a chunk's own per-column occlusion, used to
// (a) feed the +Y neighbor's contribution downward and (b) decide a chunk's
// open-top boundary when no +Y neighbor is loaded.

/// `CHUNK_SIZE × CHUNK_SIZE` per-column data for one chunk, indexed
/// `x + z * CHUNK_SIZE`.
pub type ColumnMap = Vec<u8>;

/// Index helper for a column map: `(x, z) -> x + z * CHUNK_SIZE`.
#[inline]
pub fn column_index(x: usize, z: usize) -> usize {
    x + z * CHUNK_SIZE
}

/// For each `(x, z)` column in the chunk, the local Y (0..=31) of the highest
/// sky-occluding (solid) block, or `None` if the column is entirely
/// non-solid within this chunk. Used both for the heightmap and to know
/// where skylight stops descending inside the chunk.
pub fn chunk_column_heights(chunk: &Chunk, registry: &BlockRegistry) -> Vec<Option<u8>> {
    let n = CHUNK_SIZE;
    // Uniform fast path: ~86% of streamed chunks are a single block type
    // (all-air above terrain, all-solid underground). Their column tops are
    // trivial — skip the up-to-32k-cell scan, which otherwise runs for every
    // newly loaded chunk inside the frame (a hidden streaming-burst cost).
    if chunk.is_uniform() {
        let top = if registry.is_solid(chunk.get(LocalPos::new(0, 0, 0))) {
            Some((n - 1) as u8) // all-solid: every column tops at the chunk top
        } else {
            None // all-air: no solid anywhere
        };
        return vec![top; n * n];
    }
    let mut heights = vec![None; n * n];
    for z in 0..n {
        for x in 0..n {
            let mut top: Option<u8> = None;
            // Scan from the top down; first solid is the column's height.
            for y in (0..n).rev() {
                let pos = LocalPos::new(x as u8, y as u8, z as u8);
                if registry.is_solid(chunk.get(pos)) {
                    top = Some(y as u8);
                    break;
                }
            }
            heights[column_index(x, z)] = top;
        }
    }
    heights
}

/// Whether each `(x, z)` column of this chunk has *any* solid block — i.e.
/// whether it would occlude sky for chunks below it. `true` at a column means
/// a chunk below should NOT treat that column's top as open sky.
pub fn chunk_column_occludes(chunk: &Chunk, registry: &BlockRegistry) -> Vec<bool> {
    chunk_column_heights(chunk, registry)
        .into_iter()
        .map(|h| h.is_some())
        .collect()
}

/// Read a chunk's outer-layer block-light on the given face as a 32×32 plane
/// (same indexing as [`NeighborLight`]). Used to feed a neighbor's border in
/// and to detect outgoing-border changes.
pub fn chunk_light_plane(chunk: &Chunk, face: usize) -> Vec<u8> {
    let n = CHUNK_SIZE;
    let last = (n - 1) as u8;
    let mut plane = vec![0u8; n * n];
    for v in 0..n {
        for u in 0..n {
            let (lu, lv) = (u as u8, v as u8);
            let pos = match face {
                0 => LocalPos::new(last, lu, lv), // +X: x=last, (y,z)
                1 => LocalPos::new(0, lu, lv),    // -X: x=0
                2 => LocalPos::new(lu, last, lv), // +Y: y=last, (x,z)
                3 => LocalPos::new(lu, 0, lv),    // -Y: y=0
                4 => LocalPos::new(lu, lv, last), // +Z: z=last, (x,y)
                _ => LocalPos::new(lu, lv, 0),    // -Z: z=0
            };
            plane[u + v * n] = chunk.block_light(pos);
        }
    }
    plane
}

/// Padded volume: the chunk's 32³ voxels surrounded by a one-cell border
/// (so dims are 34³). Center cells read solidity/emission from the chunk via
/// the registry and hold writable light; border cells carry neighbor light
/// (injected as "emission" so it floods inward) and are never written back.
struct PaddedChunk {
    emission: Vec<u8>, // 34³, border = neighbor light, center = block emission
    light: Vec<u8>,    // 34³ scratch
    solid: Vec<bool>,  // 34³ precomputed solidity (border shell = false)
}

const PAD: usize = CHUNK_SIZE + 2;

impl PaddedChunk {
    fn idx(x: usize, y: usize, z: usize) -> usize {
        x + y * PAD + z * PAD * PAD
    }

    fn build(
        chunk: &Chunk,
        registry: &BlockRegistry,
        borders: &NeighborLight,
        solid: Vec<bool>,
    ) -> Self {
        let n = CHUNK_SIZE;
        let mut emission = vec![0u8; PAD * PAD * PAD];

        // Center: block emission (scratch coord = local + 1).
        for pos in LocalPos::iter() {
            let e = registry.emission(chunk.get(pos));
            if e > 0 {
                let i = Self::idx(
                    pos.x() as usize + 1,
                    pos.y() as usize + 1,
                    pos.z() as usize + 1,
                );
                emission[i] = e;
            }
        }

        // Borders: inject neighbor light as emission on the matching face.
        for (face, plane) in borders.iter().enumerate() {
            let Some(plane) = plane else { continue };
            for v in 0..n {
                for u in 0..n {
                    let val = plane[u + v * n];
                    if val == 0 {
                        continue;
                    }
                    let (sx, sy, sz) = match face {
                        0 => (n + 1, u + 1, v + 1), // +X border at scratch x=n+1
                        1 => (0, u + 1, v + 1),     // -X border at x=0
                        2 => (u + 1, n + 1, v + 1), // +Y
                        3 => (u + 1, 0, v + 1),     // -Y
                        4 => (u + 1, v + 1, n + 1), // +Z
                        _ => (u + 1, v + 1, 0),     // -Z
                    };
                    emission[Self::idx(sx, sy, sz)] = val;
                }
            }
        }

        Self {
            emission,
            light: vec![0u8; PAD * PAD * PAD],
            solid,
        }
    }

    /// Map a scratch coord to the chunk LocalPos if it's a center cell.
    fn center_local(x: usize, y: usize, z: usize) -> Option<LocalPos> {
        if (1..=CHUNK_SIZE).contains(&x)
            && (1..=CHUNK_SIZE).contains(&y)
            && (1..=CHUNK_SIZE).contains(&z)
        {
            Some(LocalPos::new((x - 1) as u8, (y - 1) as u8, (z - 1) as u8))
        } else {
            None
        }
    }
}

impl LightVolume for PaddedChunk {
    fn dims(&self) -> [usize; 3] {
        [PAD, PAD, PAD]
    }
    fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        // Precomputed at build time (border shell false); no palette decode
        // in the BFS inner loop.
        self.solid[Self::idx(x, y, z)]
    }
    fn emission(&self, x: usize, y: usize, z: usize) -> u8 {
        self.emission[Self::idx(x, y, z)]
    }
    fn get_light(&self, x: usize, y: usize, z: usize) -> u8 {
        self.light[Self::idx(x, y, z)]
    }
    fn set_light(&mut self, x: usize, y: usize, z: usize, level: u8) {
        self.light[Self::idx(x, y, z)] = level;
    }
    fn accepts_spread(&self, x: usize, y: usize, z: usize) -> bool {
        // Border ring cells are light sources only — never conduits.
        Self::center_local(x, y, z).is_some()
    }
}

/// Recompute a chunk's block light from scratch, given its neighbors' border
/// light (ADR-0004). Writes the result into the chunk's light storage and
/// returns `true` if the chunk's *outgoing* border light changed (so the
/// caller can mark neighbors for relight — this is how light converges across
/// chunk boundaries).
///
/// Light is derived, not persisted: call this on load/generation and after
/// any block edit that affects light. Does not mark the chunk modified.
/// Compute a chunk's block light **without mutating it** — read-only, so it
/// can run in parallel across chunks (ADR-0004). Returns the new light as an
/// owned 32³ buffer (indexed by [`LocalPos::index`]) plus whether the
/// chunk's *outgoing* border light changed versus its current light (so the
/// caller can re-dirty neighbors for cross-chunk convergence).
///
/// Apply the buffer with [`apply_chunk_light`]. Splitting compute from apply
/// is what lets relighting be parallelized like meshing: the parallel phase
/// only reads, the cheap sequential phase writes.
pub fn compute_chunk_light(
    chunk: &Chunk,
    registry: &BlockRegistry,
    borders: &NeighborLight,
) -> (Vec<u8>, bool) {
    // Block-only convenience: sky disabled, so top_sky is irrelevant.
    let dark_top = [0u8; CHUNK_SIZE * CHUNK_SIZE];
    compute_chunk_light_2ch(
        chunk,
        registry,
        borders,
        &NEIGHBOR_SKY_NONE,
        &dark_top,
        false,
    )
}

/// A fully-open-sky top boundary (every column receiving full daylight, 15).
/// Convenience for tests and for chunks known to have nothing above them.
pub fn open_sky_top() -> Vec<u8> {
    vec![MAX_LIGHT; CHUNK_SIZE * CHUNK_SIZE]
}

/// Six neighbor sky-light planes (same shape/indexing as [`NeighborLight`]),
/// used to feed skylight across chunk boundaries. `planes[face]` is the
/// neighbor-in-that-direction's touching-face sky-light plane, or `None` if
/// not loaded.
pub type NeighborSky = [Option<Vec<u8>>; 6];

const NEIGHBOR_SKY_NONE: NeighborSky = [None, None, None, None, None, None];

/// Read a chunk's outer-layer **sky** light on the given face as a 32×32
/// plane (same indexing as [`chunk_light_plane`], which reads block light).
pub fn chunk_sky_plane(chunk: &Chunk, face: usize) -> Vec<u8> {
    let n = CHUNK_SIZE;
    let last = (n - 1) as u8;
    let mut plane = vec![0u8; n * n];
    for v in 0..n {
        for u in 0..n {
            let (lu, lv) = (u as u8, v as u8);
            let pos = match face {
                0 => LocalPos::new(last, lu, lv),
                1 => LocalPos::new(0, lu, lv),
                2 => LocalPos::new(lu, last, lv),
                3 => LocalPos::new(lu, 0, lv),
                4 => LocalPos::new(lu, lv, last),
                _ => LocalPos::new(lu, lv, 0),
            };
            plane[u + v * n] = chunk.sky_light(pos);
        }
    }
    plane
}

/// Compute both light channels for a chunk **without mutating it**, returning
/// an owned 32³ buffer of packed bytes (high nibble sky, low nibble block;
/// ADR-0004/0005) plus whether either outgoing border changed.
///
/// Block light floods from emitters + the block-light neighbor borders.
/// Skylight floods from the sky neighbor borders, with the +Y face defaulting
/// to open daylight (15) where no chunk is loaded above — so a chunk with
/// nothing overhead is sky-lit, and a loaded chunk above instead feeds its
/// own (possibly shadowed) sky downward (ADR-0005). When `do_sky` is false
/// only block light is computed (sky stays 0) — used where skylight isn't
/// wanted.
pub fn compute_chunk_light_2ch(
    chunk: &Chunk,
    registry: &BlockRegistry,
    block_borders: &NeighborLight,
    sky_borders: &NeighborSky,
    top_sky: &[u8],
    do_sky: bool,
) -> (Vec<u8>, bool) {
    // --- Fast path: uniform chunks (single block type). ---
    // ~86% of streamed chunks are uniform (all-air above terrain, or all-solid
    // underground). Their light is trivially determined, so skip both BFS
    // passes. Guard on the uniform block being a non-emitter (a uniform block
    // of lamps would need real propagation; worldgen never produces that, but
    // be safe). Note we ignore horizontal sky border spill here: a uniform
    // chunk has no faces to mesh (all-air) or admits no light (all-solid), so
    // its only role is donating border light, and the dominant donation — the
    // vertical sky column — is captured exactly. Any second-order diffuse
    // spill is resolved by the neighbor that actually needs it.
    if chunk.is_uniform() {
        let b = chunk.get(LocalPos::new(0, 0, 0));
        // Only safe to skip the block-light BFS when no block light can flood
        // in from a neighbor (e.g. an adjacent lamp lighting this air). If any
        // block border carries light, fall through to the full computation.
        let no_block_inflow = block_borders
            .iter()
            .all(|p| p.as_ref().is_none_or(|plane| plane.iter().all(|&v| v == 0)));
        // Only safe to skip the sky BFS when no neighbor sky plane brings in
        // MORE light than the heightmap's top_sky already accounts for. Below
        // the surface, real daylight flows down a shaft via the overhead (+Y)
        // plane and sideways via the horizontal planes; if any such inflow
        // exceeds top_sky, this air must propagate it — use the full path.
        let no_extra_sky_inflow = !do_sky
            || sky_borders.iter().enumerate().all(|(face, plane)| {
                // -Y (face 3) never brings sky up; ignore it.
                face == 3
                    || plane.as_ref().is_none_or(|p| {
                        p.iter().enumerate().all(|(i, &v)| {
                            // Compare against this column's top_sky where the
                            // plane indexes a column; for face planes that index
                            // (u,v) not aligned to (x,z), be conservative and
                            // require v == 0 (no inflow at all).
                            v == 0 || (matches!(face, 2) && v <= top_sky[i])
                        })
                    })
            });
        if registry.emission(b) == 0 && no_block_inflow && no_extra_sky_inflow {
            let mut out = vec![0u8; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
            if do_sky && !registry.is_solid(b) {
                // All-air: each column carries its top_sky straight down (the
                // no-attenuation-down rule keeps 15 at 15; 0 stays 0).
                for pos in LocalPos::iter() {
                    let col = pos.x() as usize + pos.z() as usize * CHUNK_SIZE;
                    out[pos.index()] = top_sky[col] << 4;
                }
            }
            // (All-solid stays fully 0: light can't enter solid cells.)
            let changed = packed_border_changed(chunk, &out);
            return (out, changed);
        }
    }

    // One padded solidity map shared by both channels (built once; the BFS
    // inner loops index it instead of palette-decoding per neighbor visit).
    let solid_map = padded_solidity(chunk, registry);

    // --- Block light (low nibble) ---
    let block = {
        let mut vol = PaddedChunk::build(chunk, registry, block_borders, solid_map.clone());
        propagate_block_light(&mut vol);
        let mut out = vec![0u8; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
        for pos in LocalPos::iter() {
            out[pos.index()] = vol.light[PaddedChunk::idx(
                pos.x() as usize + 1,
                pos.y() as usize + 1,
                pos.z() as usize + 1,
            )];
        }
        out
    };

    // --- Sky light (high nibble) ---
    let sky = if do_sky {
        compute_padded_sky(&solid_map, top_sky, sky_borders)
    } else {
        vec![0u8; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE]
    };

    // Pack: high nibble sky, low nibble block.
    let mut out = vec![0u8; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
    for i in 0..out.len() {
        out[i] = (sky[i] << 4) | (block[i] & 0x0F);
    }

    let changed = packed_border_changed(chunk, &out);
    (out, changed)
}

/// Sky-light flood over a chunk's padded volume. Seeds the border ring from
/// neighbor sky planes; the +Y border defaults to 15 (open sky) wherever no
/// neighbor-above plane is supplied. Returns the center 32³ sky buffer.
/// Sky-light flood over a chunk's padded volume.
///
/// The **top boundary** comes from `top_sky` (per-column, `CHUNK_SIZE²`,
/// indexed `x + z*CHUNK_SIZE`): the daylight entering each column from
/// directly above, derived from the world heightmap (15 where nothing solid
/// is above this chunk in the column, 0 where occluded). Using the heightmap
/// rather than the +Y neighbor's current light is what avoids a vertical
/// relight cascade — every chunk computes its correct direct daylight in one
/// pass, independent of whether its vertical neighbors have been relit yet
/// (ADR-0005).
///
/// The **horizontal** neighbor sky planes (`sky[0]`=+X, `sky[1]`=-X,
/// `sky[4]`=+Z, `sky[5]`=-Z) seed diffuse skylight spilling across side
/// chunk borders (e.g. under an overhang straddling a boundary). The ±Y
/// planes are ignored — vertical sky is handled by `top_sky` (down) and not
/// needed upward.
/// Padded (34³) solidity map for one chunk: border shell is non-solid, center
/// cells resolve through the registry ONCE. Both light channels' BFS loops
/// test this map instead of doing a palette decode (`chunk.get`) per neighbor
/// visit — the hot inner loop of relighting touches solidity ~150k+ times per
/// non-uniform chunk, so paying 32k decodes up front is a large net win.
fn padded_solidity(chunk: &Chunk, registry: &BlockRegistry) -> Vec<bool> {
    let mut solid = vec![false; PAD * PAD * PAD];
    for pos in LocalPos::iter() {
        if registry.is_solid(chunk.get(pos)) {
            solid[PaddedChunk::idx(
                pos.x() as usize + 1,
                pos.y() as usize + 1,
                pos.z() as usize + 1,
            )] = true;
        }
    }
    solid
}

fn compute_padded_sky(solid_map: &[bool], top_sky: &[u8], sky: &NeighborSky) -> Vec<u8> {
    let n = CHUNK_SIZE;
    let mut light = vec![0u8; PAD * PAD * PAD];
    let idx = PaddedChunk::idx;

    // Flat-index neighbor offsets for INTERIOR cells (x/y/z in 1..=n), for
    // which all six are always in-bounds. Layout: x + y*PAD + z*PAD².
    const OFF_X: isize = 1;
    const OFF_Y: isize = PAD as isize;
    const OFF_Z: isize = (PAD * PAD) as isize;
    const OFFS: [isize; 6] = [OFF_X, -OFF_X, OFF_Y, -OFF_Y, OFF_Z, -OFF_Z];

    // Level buckets (Dijkstra-style): each cell is finalized once, in
    // decreasing light order — no re-pushes, no per-visit allocation, no
    // coordinate math. This replaced a tuple-queue BFS that spent ~75% of
    // the whole relight cost here (allocating a Vec per popped cell in
    // `neighbors()` and re-visiting cells as better values raced in).
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); (MAX_LIGHT as usize) + 1];

    // Attempt to raise an interior cell to `val`, queueing it to spread.
    // Border ring cells are light SOURCES only, never receivers/conduits
    // (LightVolume::accepts_spread) — they are handled at seed time below
    // and never enter the buckets.
    macro_rules! try_set {
        ($i:expr, $val:expr) => {{
            let i = $i;
            let val = $val;
            if val > 0 && !solid_map[i] && light[i] < val {
                light[i] = val;
                if val > 1 {
                    buckets[val as usize].push(i as u32);
                }
            }
        }};
    }

    // --- Seeds. Values land on border-ring cells (kept for reference) and
    // are pushed INWARD immediately; the ring itself never propagates. ---
    for v in 0..n {
        for u in 0..n {
            // +X (x = n+1) / -X (x = 0): plane (y,z) = (u,v). Horizontal
            // spill enters at val-1.
            if let Some(p) = &sky[0] {
                let val = p[u + v * n];
                light[idx(n + 1, u + 1, v + 1)] = val;
                if val > 1 {
                    try_set!(idx(n, u + 1, v + 1), val - 1);
                }
            }
            if let Some(p) = &sky[1] {
                let val = p[u + v * n];
                light[idx(0, u + 1, v + 1)] = val;
                if val > 1 {
                    try_set!(idx(1, u + 1, v + 1), val - 1);
                }
            }
            // +Y top boundary (y = n+1). Two honest sources, combined by max:
            //   - the heightmap's `top_sky` (15 for columns open to sky above
            //     the natural surface — correct in one pass, no cascade), and
            //   - the REAL sky light flowing down from the chunk directly above
            //     (its -Y face plane, `sky[2]`), which carries daylight down
            //     shafts/through cave openings at/below the surface.
            // Above the surface the heightmap value dominates; below it (where
            // the heightmap reads 0) the overhead plane carries real skylight,
            // so shafts stay lit and sealed/side regions go dark via honest
            // propagation.
            let top_from_height = top_sky[u + v * n];
            let top_from_above = sky[2].as_ref().map_or(0, |p| p[u + v * n]);
            let top = top_from_height.max(top_from_above);
            light[idx(u + 1, n + 1, v + 1)] = top;
            // Full-strength daylight: fill straight down through air at 15
            // (the no-attenuation rule) as a tight column walk — this is the
            // bulk of every surface chunk's skylight and costs zero queue
            // traffic. Filled cells still enter the buckets so they spread
            // sideways (at 14) in the flood below.
            if top == MAX_LIGHT {
                let (cu, cv) = (u + 1, v + 1);
                let mut y = n;
                while y >= 1 && !solid_map[idx(cu, y, cv)] {
                    let i = idx(cu, y, cv);
                    light[i] = MAX_LIGHT;
                    buckets[MAX_LIGHT as usize].push(i as u32);
                    y -= 1;
                }
            } else if top > 1 {
                // Dimmer overhead light attenuates immediately going down.
                try_set!(idx(u + 1, n, v + 1), top - 1);
            }
            // +Z (z = n+1) / -Z (z = 0): plane (x,y) = (u,v).
            if let Some(p) = &sky[4] {
                let val = p[u + v * n];
                light[idx(u + 1, v + 1, n + 1)] = val;
                if val > 1 {
                    try_set!(idx(u + 1, v + 1, n), val - 1);
                }
            }
            if let Some(p) = &sky[5] {
                let val = p[u + v * n];
                light[idx(u + 1, v + 1, 0)] = val;
                if val > 1 {
                    try_set!(idx(u + 1, v + 1, 1), val - 1);
                }
            }
        }
    }

    // --- Flood, strictly decreasing level. Every 15 already sits atop a
    // solid or another 15 (the column fill runs to the first solid), so the
    // going-down-keeps-15 rule has no remaining work: every spread here
    // attenuates by exactly 1. Interior-only receives; the ring never
    // conducts. Popped cells whose stored level moved on are stale — skip.
    let interior = interior_mask();
    for level in (2..=MAX_LIGHT as usize).rev() {
        while let Some(i) = buckets[level].pop() {
            let i = i as usize;
            if light[i] as usize != level {
                continue;
            }
            let spread = (level - 1) as u8;
            for off in OFFS {
                let ni = (i as isize + off) as usize;
                if !interior[ni] || solid_map[ni] {
                    continue;
                }
                if light[ni] < spread {
                    light[ni] = spread;
                    if spread > 1 {
                        buckets[spread as usize].push(ni as u32);
                    }
                }
            }
        }
    }

    // Extract center 32³.
    let mut out = vec![0u8; n * n * n];
    for pos in LocalPos::iter() {
        out[pos.index()] = light[PaddedChunk::idx(
            pos.x() as usize + 1,
            pos.y() as usize + 1,
            pos.z() as usize + 1,
        )];
    }
    out
}

/// Static mask of interior (non-border) cells of the padded volume; border
/// ring cells never receive spread (sources only).
fn interior_mask() -> &'static [bool] {
    use std::sync::OnceLock;
    static MASK: OnceLock<Vec<bool>> = OnceLock::new();
    MASK.get_or_init(|| {
        let mut m = vec![false; PAD * PAD * PAD];
        for z in 1..=CHUNK_SIZE {
            for y in 1..=CHUNK_SIZE {
                for x in 1..=CHUNK_SIZE {
                    m[PaddedChunk::idx(x, y, z)] = true;
                }
            }
        }
        m
    })
}

/// Apply a packed light buffer (from [`compute_chunk_light_2ch`]) to a
/// chunk's storage: high nibble → sky light, low nibble → block light.
pub fn apply_chunk_light(chunk: &mut Chunk, light: &[u8]) {
    chunk.clear_light();
    for pos in LocalPos::iter() {
        let packed = light[pos.index()];
        let block = packed & 0x0F;
        let sky = (packed >> 4) & 0x0F;
        if block > 0 {
            chunk.set_block_light(pos, block);
        }
        if sky > 0 {
            chunk.set_sky_light(pos, sky);
        }
    }
}

/// Whether the outgoing border planes of a packed `new_light` (32³) buffer
/// differ from the chunk's currently-stored packed light on any of the six
/// faces — checking both channels at once.
/// Bitmask of which of the six faces' outgoing border planes differ between
/// `new_light` (packed 32³) and the chunk's stored packed light. Bit `i` set
/// means face `i` changed, in the canonical face order +X, -X, +Y, -Y, +Z, -Z
/// (matching `NeighborLight`/`NeighborSky` indexing). Lets a caller requeue
/// only the neighbor across a changed face instead of all six — during
/// streaming convergence that cuts relight/remesh cascade fanout by up to 6x.
pub fn packed_border_changed_faces(chunk: &Chunk, new_light: &[u8]) -> u8 {
    let n = CHUNK_SIZE;
    let last = (n - 1) as u8;
    let idx = |x: u8, y: u8, z: u8| LocalPos::new(x, y, z).index();
    let current = |i: usize| {
        let p = LocalPos::from_index(i);
        (chunk.sky_light(p) << 4) | (chunk.block_light(p) & 0x0F)
    };
    let mut mask = 0u8;
    for v in 0..n {
        for u in 0..n {
            let (lu, lv) = (u as u8, v as u8);
            let faces = [
                idx(last, lu, lv),
                idx(0, lu, lv),
                idx(lu, last, lv),
                idx(lu, 0, lv),
                idx(lu, lv, last),
                idx(lu, lv, 0),
            ];
            for (face, &i) in faces.iter().enumerate() {
                if mask & (1 << face) == 0 && current(i) != new_light[i] {
                    mask |= 1 << face;
                }
            }
        }
    }
    mask
}

fn packed_border_changed(chunk: &Chunk, new_light: &[u8]) -> bool {
    let n = CHUNK_SIZE;
    let last = (n - 1) as u8;
    let idx = |x: u8, y: u8, z: u8| LocalPos::new(x, y, z).index();
    let current = |i: usize| {
        let p = LocalPos::from_index(i);
        (chunk.sky_light(p) << 4) | (chunk.block_light(p) & 0x0F)
    };
    for v in 0..n {
        for u in 0..n {
            let (lu, lv) = (u as u8, v as u8);
            let faces = [
                idx(last, lu, lv),
                idx(0, lu, lv),
                idx(lu, last, lv),
                idx(lu, 0, lv),
                idx(lu, lv, last),
                idx(lu, lv, 0),
            ];
            for &i in &faces {
                if current(i) != new_light[i] {
                    return true;
                }
            }
        }
    }
    false
}

/// Recompute a chunk's **block light only**, in place (compute + apply).
/// Convenience for single-chunk use and tests; preserves M04 semantics
/// (no skylight). The streaming path uses [`compute_chunk_light_2ch`] /
/// [`apply_chunk_light`] to compute both channels in parallel.
pub fn relight_chunk(chunk: &mut Chunk, registry: &BlockRegistry, borders: &NeighborLight) -> bool {
    let dark_top = [0u8; CHUNK_SIZE * CHUNK_SIZE];
    let (light, changed) = compute_chunk_light_2ch(
        chunk,
        registry,
        borders,
        &NEIGHBOR_SKY_NONE,
        &dark_top,
        false,
    );
    apply_chunk_light(chunk, &light);
    changed
}

/// Recompute **both** light channels for a chunk in place (compute + apply),
/// with a fully-open sky above (every column daylit). Convenience wrapper for
/// tests; the streaming path supplies a real heightmap-derived `top_sky`.
pub fn relight_chunk_2ch(
    chunk: &mut Chunk,
    registry: &BlockRegistry,
    block_borders: &NeighborLight,
    sky_borders: &NeighborSky,
) -> bool {
    let top = open_sky_top();
    let (light, changed) =
        compute_chunk_light_2ch(chunk, registry, block_borders, sky_borders, &top, true);
    apply_chunk_light(chunk, &light);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple dense test volume: per-cell solid flag + emission + light.
    struct Grid {
        dims: [usize; 3],
        solid: Vec<bool>,
        emission: Vec<u8>,
        light: Vec<u8>,
    }
    impl Grid {
        fn new(dx: usize, dy: usize, dz: usize) -> Self {
            let n = dx * dy * dz;
            Self {
                dims: [dx, dy, dz],
                solid: vec![false; n],
                emission: vec![0; n],
                light: vec![0; n],
            }
        }
        fn idx(&self, x: usize, y: usize, z: usize) -> usize {
            x + y * self.dims[0] + z * self.dims[0] * self.dims[1]
        }
        fn set_solid(&mut self, x: usize, y: usize, z: usize) {
            let i = self.idx(x, y, z);
            self.solid[i] = true;
        }
        #[allow(dead_code)]
        fn clear_solid(&mut self, x: usize, y: usize, z: usize) {
            let i = self.idx(x, y, z);
            self.solid[i] = false;
        }
        fn set_emission(&mut self, x: usize, y: usize, z: usize, e: u8) {
            let i = self.idx(x, y, z);
            self.emission[i] = e;
        }
    }
    impl LightVolume for Grid {
        fn dims(&self) -> [usize; 3] {
            self.dims
        }
        fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
            self.solid[self.idx(x, y, z)]
        }
        fn emission(&self, x: usize, y: usize, z: usize) -> u8 {
            self.emission[self.idx(x, y, z)]
        }
        fn get_light(&self, x: usize, y: usize, z: usize) -> u8 {
            self.light[self.idx(x, y, z)]
        }
        fn set_light(&mut self, x: usize, y: usize, z: usize, level: u8) {
            let i = self.idx(x, y, z);
            self.light[i] = level;
        }
    }

    #[test]
    fn single_source_radial_falloff() {
        // Source at center of an open 11³ grid emits 15; light should fall
        // off by Manhattan distance.
        let mut g = Grid::new(11, 11, 11);
        g.set_emission(5, 5, 5, 15);
        propagate_block_light(&mut g);

        assert_eq!(g.get_light(5, 5, 5), 15);
        assert_eq!(g.get_light(6, 5, 5), 14); // 1 step
        assert_eq!(g.get_light(5, 5, 7), 13); // 2 steps
        // A cell 5 steps away in a straight line.
        assert_eq!(g.get_light(10, 5, 5), 10);
        // Manhattan distance 15 would be level 0; distance 10 → 5.
        assert_eq!(g.get_light(10, 10, 5), 15 - 10);
    }

    #[test]
    fn corridor_linear_falloff() {
        // A 1-wide corridor: walls solid, a 1×1×N tube of air. Light falls
        // off linearly along the only open path.
        let mut g = Grid::new(20, 3, 3);
        // Make everything solid, then carve the row y=1,z=1.
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..20 {
                    if !(y == 1 && z == 1) {
                        g.set_solid(x, y, z);
                    }
                }
            }
        }
        g.set_emission(0, 1, 1, 15);
        propagate_block_light(&mut g);

        for x in 0..16 {
            assert_eq!(g.get_light(x, 1, 1), (15 - x as u8), "at x={x}");
        }
        assert_eq!(g.get_light(15, 1, 1), 0);
    }

    #[test]
    fn light_stops_at_solid_wall() {
        // Source on one side of a solid wall; the far side stays dark.
        let mut g = Grid::new(5, 3, 3);
        // Wall at x=2 spanning the whole y,z cross-section.
        for z in 0..3 {
            for y in 0..3 {
                g.set_solid(2, y, z);
            }
        }
        g.set_emission(0, 1, 1, 15);
        propagate_block_light(&mut g);

        assert!(g.get_light(1, 1, 1) > 0, "near side lit");
        assert_eq!(g.get_light(2, 1, 1), 0, "wall itself unlit (no emission)");
        assert_eq!(g.get_light(3, 1, 1), 0, "far side dark");
        assert_eq!(g.get_light(4, 1, 1), 0);
    }

    #[test]
    fn overlapping_sources_take_max_not_sum() {
        // Two emitters; a cell between them takes the brighter contribution.
        let mut g = Grid::new(11, 3, 3);
        g.set_emission(0, 1, 1, 15);
        g.set_emission(10, 1, 1, 15);
        propagate_block_light(&mut g);

        // Midpoint x=5 is 5 from each: each contributes 10, max = 10 (not 20).
        assert_eq!(g.get_light(5, 1, 1), 10);
        assert!(g.get_light(5, 1, 1) <= MAX_LIGHT);
    }

    #[test]
    fn emitter_can_be_solid() {
        // A glowing solid block (like a lamp): it holds its emission and
        // lights its non-solid neighbors.
        let mut g = Grid::new(5, 3, 3);
        g.set_solid(2, 1, 1);
        g.set_emission(2, 1, 1, 14);
        propagate_block_light(&mut g);

        assert_eq!(g.get_light(2, 1, 1), 14, "solid emitter holds emission");
        assert_eq!(g.get_light(1, 1, 1), 13, "neighbor lit one less");
        assert_eq!(g.get_light(3, 1, 1), 13);
    }

    #[test]
    fn empty_volume_stays_dark() {
        let mut g = Grid::new(8, 8, 8);
        let lit = propagate_block_light(&mut g);
        assert_eq!(lit, 0);
        assert_eq!(g.get_light(4, 4, 4), 0);
    }

    #[test]
    fn recompute_clears_stale_light() {
        // Pre-seed bogus light, then propagate with no emitters: must clear.
        let mut g = Grid::new(5, 5, 5);
        g.set_light(2, 2, 2, 9);
        let lit = propagate_block_light(&mut g);
        assert_eq!(lit, 0, "stale light must be cleared on recompute");
        assert_eq!(g.get_light(2, 2, 2), 0);
    }

    #[test]
    fn level_one_does_not_propagate() {
        // A source emitting 1 lights only itself (1-1 = 0 to neighbors).
        let mut g = Grid::new(5, 5, 5);
        g.set_emission(2, 2, 2, 1);
        propagate_block_light(&mut g);
        assert_eq!(g.get_light(2, 2, 2), 1);
        assert_eq!(g.get_light(3, 2, 2), 0);
    }

    #[test]
    fn light_goes_around_obstacles() {
        // A partial wall with a gap: light should bend around through the gap.
        let mut g = Grid::new(7, 7, 3);
        // Wall at x=3 for all y except a gap at y=0.
        for z in 0..3 {
            for y in 1..7 {
                g.set_solid(3, y, z);
            }
        }
        g.set_emission(0, 3, 1, 15);
        propagate_block_light(&mut g);

        // Directly behind the wall but not at the gap row: only reachable by
        // routing through the gap, so dimmer than the straight-line distance.
        let behind = g.get_light(4, 3, 1);
        assert!(behind > 0, "light should reach behind via the gap");
        // The blocked cell on the wall is dark.
        assert_eq!(g.get_light(3, 3, 1), 0);
    }

    // ---- Sky light propagation (Milestone 05) ----

    /// Open sky over every column (all 15 incoming).
    fn open_top(dx: usize, dz: usize) -> Vec<u8> {
        vec![15u8; dx * dz]
    }

    #[test]
    fn sky_open_flat_is_all_15() {
        // Empty volume, open sky everywhere → every cell full daylight.
        let mut g = Grid::new(5, 5, 5);
        propagate_sky_light(&mut g, &open_top(5, 5));
        for z in 0..5 {
            for y in 0..5 {
                for x in 0..5 {
                    assert_eq!(g.get_light(x, y, z), 15, "open air cell {x},{y},{z}");
                }
            }
        }
    }

    #[test]
    fn sky_vertical_shaft_stays_15_to_floor() {
        // Solid everywhere except a 1-wide vertical shaft of air at (2,*,2).
        let mut g = Grid::new(5, 10, 5);
        for z in 0..5 {
            for y in 0..10 {
                for x in 0..5 {
                    if !(x == 2 && z == 2) {
                        g.set_solid(x, y, z);
                    }
                }
            }
        }
        propagate_sky_light(&mut g, &open_top(5, 5));
        // No attenuation straight down: full 15 from top to floor.
        for y in 0..10 {
            assert_eq!(g.get_light(2, y, 2), 15, "shaft at y={y}");
        }
    }

    #[test]
    fn sky_horizontal_tunnel_darkens() {
        // A vertical shaft, then a horizontal tunnel branching off its bottom.
        let mut g = Grid::new(12, 8, 3);
        // Fill solid, carve shaft at x=0 (all y, z=1) and a tunnel at y=0.
        for z in 0..3 {
            for y in 0..8 {
                for x in 0..12 {
                    g.set_solid(x, y, z);
                }
            }
        }
        // Shaft: x=0, z=1, all y open.
        for y in 0..8 {
            g.clear_solid(0, y, 1);
        }
        // Tunnel along x at y=0, z=1.
        for x in 0..12 {
            g.clear_solid(x, 0, 1);
        }
        propagate_sky_light(&mut g, &open_top(12, 3));

        // Shaft bottom is 15 (came straight down).
        assert_eq!(g.get_light(0, 0, 1), 15);
        // Tunnel darkens by one per horizontal step away from the shaft.
        assert_eq!(g.get_light(1, 0, 1), 14);
        assert_eq!(g.get_light(2, 0, 1), 13);
        assert_eq!(g.get_light(5, 0, 1), 10);
    }

    #[test]
    fn sky_under_roof_is_shadowed() {
        // A solid roof over an air cavity, with solid walls except open sky
        // above the roof. The cavity beneath gets no direct sky.
        let mut g = Grid::new(5, 5, 5);
        // Roof slab at y=3 across the whole xz.
        for z in 0..5 {
            for x in 0..5 {
                g.set_solid(x, 3, z);
            }
        }
        propagate_sky_light(&mut g, &open_top(5, 5));
        // Above the roof: lit.
        assert_eq!(g.get_light(2, 4, 2), 15);
        // Below the roof, sealed from the sides: dark.
        assert_eq!(g.get_light(2, 2, 2), 0);
        assert_eq!(g.get_light(2, 0, 2), 0);
    }

    #[test]
    fn sky_roof_with_side_opening_lets_diffuse_in() {
        // Roof at y=3, but the volume's sides are open (no walls), so daylight
        // from the open columns beside the roof spills under it horizontally.
        let mut g = Grid::new(7, 5, 3);
        // Roof slab only covers x=2..=4 at y=3; rest open to sky.
        for z in 0..3 {
            for x in 2..=4 {
                g.set_solid(x, 3, z);
            }
        }
        propagate_sky_light(&mut g, &open_top(7, 3));
        // Open column beside the roof is full daylight top-to-bottom.
        assert_eq!(g.get_light(0, 0, 1), 15);
        // Under the roof edge: lit by diffuse spill from the open side, dimmer.
        let under_edge = g.get_light(2, 2, 1);
        assert!(
            under_edge > 0 && under_edge < 15,
            "diffuse under roof edge: {under_edge}"
        );
        // Deeper under the roof center: dimmer still (or dark).
        assert!(g.get_light(4, 2, 1) <= under_edge);
    }

    #[test]
    fn sky_partial_top_boundary_occludes_some_columns() {
        // Simulate a neighbor above occluding half the columns: top_sky = 0
        // for x<3 (covered), 15 for x>=3 (open).
        let mut g = Grid::new(6, 4, 2);
        let mut top = vec![0u8; 6 * 2];
        for z in 0..2 {
            for x in 3..6 {
                top[x + z * 6] = 15;
            }
        }
        propagate_sky_light(&mut g, &top);
        // Open side: full daylight.
        assert_eq!(g.get_light(5, 3, 0), 15);
        // Covered side top cell: no direct sky (0 incoming), only diffuse from
        // the open side — so dimmer than 15.
        assert!(g.get_light(0, 3, 0) < 15);
    }

    // ---- Removal (two-phase) ----

    /// Helper: full add, then remove at a cell whose emission was zeroed.
    fn add_then_remove_at(g: &mut Grid, rx: usize, ry: usize, rz: usize) {
        propagate_block_light(g);
        // Simulate breaking the source: clear its emission, then remove.
        g.set_emission(rx, ry, rz, 0);
        remove_block_light(g, rx, ry, rz);
    }

    #[test]
    fn removing_only_source_restores_darkness() {
        let mut g = Grid::new(11, 11, 11);
        g.set_emission(5, 5, 5, 15);
        add_then_remove_at(&mut g, 5, 5, 5);

        // Everything must be dark again — no stale glow anywhere.
        for z in 0..11 {
            for y in 0..11 {
                for x in 0..11 {
                    assert_eq!(g.get_light(x, y, z), 0, "stale light at {x},{y},{z}");
                }
            }
        }
    }

    #[test]
    fn removing_one_of_two_sources_keeps_survivor_light() {
        // THE case naive removal gets wrong: two overlapping sources, remove
        // one, the region must remain correctly lit by the other.
        let mut g = Grid::new(11, 3, 3);
        g.set_emission(0, 1, 1, 15);
        g.set_emission(10, 1, 1, 15);
        add_then_remove_at(&mut g, 0, 1, 1); // remove the left source

        // Compare against a from-scratch recompute with only the right source.
        let mut reference = Grid::new(11, 3, 3);
        reference.set_emission(10, 1, 1, 15);
        propagate_block_light(&mut reference);

        for x in 0..11 {
            assert_eq!(
                g.get_light(x, 1, 1),
                reference.get_light(x, 1, 1),
                "mismatch at x={x} after removing one source"
            );
        }
        // Sanity: the surviving source is still full bright, far end dark-ish.
        assert_eq!(g.get_light(10, 1, 1), 15);
        assert_eq!(g.get_light(0, 1, 1), 5); // 10 away from survivor
    }

    #[test]
    fn removal_matches_full_recompute_random() {
        // Fuzz: random emitters + walls, remove one, compare to a clean
        // recompute without it. Removal must converge to the same field.
        let mut rng = 0x1234_5678u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for _ in 0..40 {
            let mut g = Grid::new(9, 9, 9);
            // Sprinkle walls.
            for _ in 0..60 {
                let x = (next() % 9) as usize;
                let y = (next() % 9) as usize;
                let z = (next() % 9) as usize;
                g.set_solid(x, y, z);
            }
            // A few emitters on non-solid cells.
            let mut emitters = Vec::new();
            for _ in 0..4 {
                let x = (next() % 9) as usize;
                let y = (next() % 9) as usize;
                let z = (next() % 9) as usize;
                if !g.is_solid(x, y, z) {
                    let e = 8 + (next() % 8) as u8;
                    g.set_emission(x, y, z, e);
                    emitters.push((x, y, z));
                }
            }
            if emitters.is_empty() {
                continue;
            }
            propagate_block_light(&mut g);

            // Remove the first emitter.
            let (rx, ry, rz) = emitters[0];
            g.set_emission(rx, ry, rz, 0);
            remove_block_light(&mut g, rx, ry, rz);

            // Reference: recompute from scratch with that emitter gone.
            let mut reference = Grid::new(9, 9, 9);
            reference.solid = g.solid.clone();
            reference.emission = g.emission.clone();
            propagate_block_light(&mut reference);

            for i in 0..(9 * 9 * 9) {
                assert_eq!(
                    g.light[i], reference.light[i],
                    "incremental removal diverged from full recompute"
                );
            }
        }
    }

    #[test]
    fn placing_solid_over_light_removes_it() {
        // Light a corridor, then drop a solid wall mid-way and remove from
        // there: the far side past the new wall goes dark.
        let mut g = Grid::new(12, 3, 3);
        for z in 0..3 {
            for y in 0..3 {
                for x in 0..12 {
                    if !(y == 1 && z == 1) {
                        g.set_solid(x, y, z);
                    }
                }
            }
        }
        g.set_emission(0, 1, 1, 15);
        propagate_block_light(&mut g);
        assert!(g.get_light(8, 1, 1) > 0);

        // Place a solid block at x=5 and remove light there.
        g.set_solid(5, 1, 1);
        remove_block_light(&mut g, 5, 1, 1);

        // Near side still lit, far side past the wall dark.
        assert!(g.get_light(4, 1, 1) > 0, "near side stays lit");
        assert_eq!(g.get_light(5, 1, 1), 0, "the new solid is dark");
        let reference_far = g.get_light(8, 1, 1);
        assert_eq!(reference_far, 0, "far side past new wall went dark");
    }

    // ---- Chunk relight adapter ----

    use crate::block::BlockId;
    use crate::registry::{BlockRegistry, LAMP};

    fn no_borders() -> NeighborLight {
        [None, None, None, None, None, None]
    }

    #[test]
    fn relight_empty_chunk_is_dark() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        let changed = relight_chunk(&mut chunk, &reg, &no_borders());
        assert!(!chunk.has_light(), "no emitters → fully dark");
        assert!(!changed, "no outgoing border change");
    }

    #[test]
    fn relight_lamp_lights_neighborhood() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Lamp in the interior so falloff is visible without borders.
        let lp = LocalPos::new(16, 16, 16);
        chunk.set(lp, LAMP);
        relight_chunk(&mut chunk, &reg, &no_borders());

        assert_eq!(chunk.block_light(lp), 14, "lamp holds its emission");
        assert_eq!(chunk.block_light(LocalPos::new(17, 16, 16)), 13);
        assert_eq!(chunk.block_light(LocalPos::new(18, 16, 16)), 12);
        // Far corner well beyond reach stays dark.
        assert_eq!(chunk.block_light(LocalPos::new(0, 0, 0)), 0);
    }

    #[test]
    fn relight_changes_border_when_light_reaches_edge() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Lamp near the -X edge so light reaches x=0.
        chunk.set(LocalPos::new(2, 16, 16), LAMP);
        let changed = relight_chunk(&mut chunk, &reg, &no_borders());
        assert!(
            changed,
            "light reaching the edge changes the outgoing border"
        );
        // The -X plane (x=0) at the lamp's row should be lit.
        let plane = chunk_light_plane(&chunk, 1); // NEG_X
        assert!(
            plane[16 + 16 * CHUNK_SIZE] > 0,
            "edge plane lit at lamp row"
        );
    }

    #[test]
    fn border_light_flows_in_from_neighbor() {
        // Simulate a bright neighbor on -X: inject a full-bright border plane
        // and confirm light floods into this (otherwise dark) chunk.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();

        let n = CHUNK_SIZE;
        let mut neg_x_plane = vec![0u8; n * n];
        // Neighbor's touching plane is bright (15) at one row.
        neg_x_plane[16 + 16 * n] = 15;
        let borders: NeighborLight = [None, Some(neg_x_plane), None, None, None, None];

        relight_chunk(&mut chunk, &reg, &borders);

        // The -X edge cell (x=0) at that row receives 15-1=14 (one step in).
        assert_eq!(chunk.block_light(LocalPos::new(0, 16, 16)), 14);
        assert_eq!(chunk.block_light(LocalPos::new(1, 16, 16)), 13);
    }

    #[test]
    fn relight_clears_stale_light_when_lamp_removed() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        let lp = LocalPos::new(16, 16, 16);
        chunk.set(lp, LAMP);
        relight_chunk(&mut chunk, &reg, &no_borders());
        assert!(chunk.has_light());

        // Remove the lamp and relight from scratch.
        chunk.set(lp, BlockId::AIR);
        relight_chunk(&mut chunk, &reg, &no_borders());
        assert!(
            !chunk.has_light(),
            "removing the only lamp clears all light"
        );
    }

    // ---- Two-channel relight (Milestone 05 task 3) ----

    fn no_sky() -> NeighborSky {
        [None, None, None, None, None, None]
    }

    #[test]
    fn two_channel_open_air_chunk_is_fully_skylit() {
        // An all-air chunk with no neighbors → open top → sky 15 everywhere,
        // block light 0 everywhere.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        relight_chunk_2ch(&mut chunk, &reg, &no_borders(), &no_sky());
        for pos in [
            LocalPos::new(0, 0, 0),
            LocalPos::new(31, 31, 31),
            LocalPos::new(16, 0, 16),
        ] {
            assert_eq!(chunk.sky_light(pos), 15, "open air sky at {pos:?}");
            assert_eq!(chunk.block_light(pos), 0, "no block light");
        }
    }

    #[test]
    fn two_channel_lamp_and_sky_coexist() {
        // A lamp in open air: block light from the lamp AND full skylight, in
        // separate nibbles, both present.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        let lp = LocalPos::new(16, 16, 16);
        chunk.set(lp, LAMP);
        relight_chunk_2ch(&mut chunk, &reg, &no_borders(), &no_sky());

        // Lamp cell: block light = emission; sky is 0 there (lamp is solid).
        assert_eq!(chunk.block_light(lp), 14);
        assert_eq!(chunk.sky_light(lp), 0, "solid lamp blocks sky in its cell");
        // A nearby air cell: both channels lit.
        let near = LocalPos::new(17, 16, 16);
        assert_eq!(chunk.block_light(near), 13);
        assert_eq!(chunk.sky_light(near), 15);
    }

    #[test]
    fn two_channel_roof_casts_sky_shadow_not_block_shadow() {
        // A solid roof slab high in the chunk; cells below are sky-shadowed.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Roof at y=20 across the whole chunk.
        for z in 0..32u8 {
            for x in 0..32u8 {
                chunk.set(LocalPos::new(x, 20, z), STONE);
            }
        }
        relight_chunk_2ch(&mut chunk, &reg, &no_borders(), &no_sky());

        // Above the roof: full sky.
        assert_eq!(chunk.sky_light(LocalPos::new(16, 25, 16)), 15);
        // Directly below the roof (sealed): no sky.
        assert_eq!(chunk.sky_light(LocalPos::new(16, 10, 16)), 0);
        assert_eq!(chunk.sky_light(LocalPos::new(16, 0, 16)), 0);
    }

    #[test]
    fn two_channel_sky_flows_down_from_neighbor_above() {
        // Open top (top_sky = 15 per column) → daylight floods down a clear
        // chunk and, with the no-down-attenuation rule, stays 15 to the floor.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let top = open_sky_top();
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        let sky_at = |p: LocalPos| (light[p.index()] >> 4) & 0x0F;
        assert_eq!(sky_at(LocalPos::new(16, 31, 16)), 15);
        assert_eq!(sky_at(LocalPos::new(16, 0, 16)), 15);
    }

    #[test]
    fn two_channel_occluded_top_blocks_sky() {
        // top_sky = 0 for every column (the heightmap says something solid is
        // above this chunk) → no daylight enters; with no other source the
        // chunk is sky-dark.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let dark_top = vec![0u8; CHUNK_SIZE * CHUNK_SIZE];
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &dark_top, true);
        let sky_at = |p: LocalPos| (light[p.index()] >> 4) & 0x0F;
        assert_eq!(
            sky_at(LocalPos::new(16, 31, 16)),
            0,
            "occluded top (heightmap) => shadow"
        );
    }

    #[test]
    fn uniform_fast_path_matches_full_for_open_air() {
        // An all-air chunk under open sky: the fast path fills each column
        // with top_sky straight down. A partial top_sky (some columns lit,
        // some occluded) must produce exactly that pattern, 15 or 0.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let mut top = vec![0u8; n * n];
        for z in 0..n {
            for x in 0..n {
                if x >= n / 2 {
                    top[x + z * n] = 15;
                }
            }
        }
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        for pos in LocalPos::iter() {
            let sky = (light[pos.index()] >> 4) & 0x0F;
            let want = if (pos.x() as usize) >= n / 2 { 15 } else { 0 };
            assert_eq!(sky, want, "air column sky at {pos:?}");
            assert_eq!(light[pos.index()] & 0x0F, 0, "no block light in open air");
        }
    }

    #[test]
    fn uniform_fast_path_solid_is_dark() {
        // An all-stone chunk: light cannot enter; every cell stays 0 in both
        // channels regardless of top_sky.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::filled(STONE);
        let (light, _) = compute_chunk_light_2ch(
            &chunk,
            &reg,
            &no_borders(),
            &no_sky(),
            &open_sky_top(),
            true,
        );
        assert!(light.iter().all(|&v| v == 0), "solid chunk fully dark");
    }

    #[test]
    fn uniform_with_block_inflow_uses_full_path() {
        // An all-air chunk WITH a block-light border must still flood that
        // block light in (fast path must defer to the full computation).
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let mut neg_x = vec![0u8; n * n];
        neg_x[16 + 16 * n] = 15;
        let block_borders: NeighborLight = [None, Some(neg_x), None, None, None, None];
        let dark_top = vec![0u8; n * n];
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &block_borders, &no_sky(), &dark_top, true);
        // Block light floods in from the -X border.
        assert_eq!(light[LocalPos::new(0, 16, 16).index()] & 0x0F, 14);
    }

    #[test]
    fn platform_casts_shadow_underneath() {
        use crate::*;
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Solid platform across the whole chunk at y=20 (so no open sides — fully enclosed below).
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                chunk.set(LocalPos::new(x, 20, z), STONE);
            }
        }
        let top = open_sky_top(); // heightmap: column open (platform is top solid)
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        let sky = |x, y, z| (light[LocalPos::new(x, y, z).index()] >> 4) & 0x0F;
        println!("above platform y=25: {}", sky(16, 25, 16));
        println!("just below platform y=19: {}", sky(16, 19, 16));
        println!("bottom y=0: {}", sky(16, 0, 16));
        assert_eq!(
            sky(16, 25, 16),
            15,
            "above platform should be full daylight"
        );
        assert_eq!(
            sky(16, 19, 16),
            0,
            "fully-covered cell below platform must be dark"
        );
    }

    // ---- Heightmap / sky occlusion (Milestone 05) ----

    use crate::registry::STONE;

    #[test]
    fn empty_chunk_has_no_column_heights() {
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let h = chunk_column_heights(&chunk, &reg);
        assert!(h.iter().all(|c| c.is_none()), "air columns are open");
        let occ = chunk_column_occludes(&chunk, &reg);
        assert!(occ.iter().all(|&o| !o));
    }

    #[test]
    fn column_height_is_topmost_solid() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Stack of stone at (4, z=5) from y=0..=10; plus a floating block at 20.
        for y in 0..=10u8 {
            chunk.set(LocalPos::new(4, y, 5), STONE);
        }
        chunk.set(LocalPos::new(4, 20, 5), STONE);
        let h = chunk_column_heights(&chunk, &reg);
        assert_eq!(h[column_index(4, 5)], Some(20), "topmost solid wins");
        // A neighbor column with only the low stack.
        assert_eq!(h[column_index(0, 0)], None);
    }

    #[test]
    fn full_floor_occludes_every_column() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        // Solid slab at y=0 across the whole chunk.
        for z in 0..32u8 {
            for x in 0..32u8 {
                chunk.set(LocalPos::new(x, 0, z), STONE);
            }
        }
        let occ = chunk_column_occludes(&chunk, &reg);
        assert!(occ.iter().all(|&o| o), "every column occludes");
        let h = chunk_column_heights(&chunk, &reg);
        assert!(h.iter().all(|&c| c == Some(0)));
    }

    #[test]
    fn air_block_does_not_occlude() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(1, 1, 1), BlockId::AIR); // no-op
        let occ = chunk_column_occludes(&chunk, &reg);
        assert!(!occ[column_index(1, 1)]);
    }
    #[test]
    fn enclosed_box_interior_is_dark() {
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (x, y, z) = ((16 + dx) as u8, (16 + dy) as u8, (16 + dz) as u8);
                    if dx.abs() == 1 || dy.abs() == 1 || dz.abs() == 1 {
                        chunk.set(LocalPos::new(x, y, z), STONE);
                    }
                }
            }
        }
        let top = open_sky_top();
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        let center = (light[LocalPos::new(16, 16, 16).index()] >> 4) & 0x0F;
        println!("enclosed box interior sky: {center}");
        assert_eq!(center, 0, "sealed box interior must be pitch dark");
    }

    #[test]
    fn fast_path_air_chunk_under_platform_in_heightmap() {
        // The bug repro: an all-air chunk sits BELOW a platform that lives in
        // the chunk above. The heightmap (top_sky) for THIS air chunk's
        // columns should say "occluded" (0) because the platform above blocks
        // the sky. If the app passes top_sky=0, the fast path must produce a
        // DARK chunk. If instead top_sky=15 is passed (heightmap stale/wrong),
        // the fast path floods it with daylight -> the observed bug.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air(); // uniform all-air -> hits fast path
        // Correct heightmap: platform above => this column is occluded.
        let occluded_top = vec![0u8; CHUNK_SIZE * CHUNK_SIZE];
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &occluded_top, true);
        let center = (light[LocalPos::new(16, 16, 16).index()] >> 4) & 0x0F;
        println!("air-under-platform (top_sky=0) center sky: {center}");
        assert_eq!(
            center, 0,
            "air chunk under a platform must be dark when top_sky=0"
        );
    }

    #[test]
    fn overhead_plane_lights_shaft_below_surface() {
        // Below-surface chunk (top_sky all 0, i.e. heightmap says "covered"),
        // but the chunk ABOVE passes real daylight down its -Y plane at one
        // column (a shaft). That column must light up 15 top-to-bottom (no-down
        // attenuation); other columns stay dark. This is honest skylight, not
        // the heightmap.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let mut above = vec![0u8; n * n]; // overhead chunk's -Y sky plane
        let shaft = 16 + 16 * n;
        above[shaft] = 15; // one open column directly overhead
        let sky: NeighborSky = [None, None, Some(above), None, None, None];
        let dark_top = vec![0u8; n * n]; // heightmap: covered
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &sky, &dark_top, true);
        let sky_at = |x, y, z| (light[LocalPos::new(x, y, z).index()] >> 4) & 0x0F;
        assert_eq!(sky_at(16, 31, 16), 15, "shaft top lit from overhead plane");
        assert_eq!(sky_at(16, 0, 16), 15, "shaft stays 15 to the bottom");
        // A column far from the shaft only gets attenuated horizontal spread.
        assert!(sky_at(16 + 5, 31, 16) < 15, "off-shaft column dimmer");
        assert!(sky_at(16 + 14, 16, 16) <= 1, "far from shaft ~dark");
    }

    #[test]
    fn covered_air_chunk_below_surface_is_dark() {
        // Below-surface air chunk, overhead plane all 0 (solid roof above) and
        // top_sky all 0: must be fully dark. Confirms we do not invent skylight.
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let above = vec![0u8; n * n];
        let sky: NeighborSky = [None, None, Some(above), None, None, None];
        let dark_top = vec![0u8; n * n];
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &sky, &dark_top, true);
        assert!(
            light.iter().all(|&v| (v >> 4) == 0),
            "covered air stays dark"
        );
    }

    #[test]
    fn upward_tunnel_stops_below_surface() {
        // Surface at y=20 (stone 0..=20, air above). Carve a vertical air
        // shaft from y=1 up to y=18 (top still has solid at y=19,20 above it).
        // With top_sky=15 (heightmap: column open above the y=20 surface),
        // daylight enters at the chunk top and floods DOWN, correctly stopping
        // at y=20. The shaft (y<=18) is sealed beneath two solid blocks, so it
        // must stay DARK. If it lights up, top_sky/propagation is leaking.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                for y in 0..=20u8 {
                    chunk.set(LocalPos::new(x, y, z), STONE);
                }
            }
        }
        // Carve the shaft at column (16,16): clear y=1..=18 (leave 19,20 solid).
        for y in 1..=18u8 {
            chunk.set(LocalPos::new(16, y, 16), BlockId::AIR);
        }
        let top = open_sky_top(); // top_sky = 15 everywhere (open above surface)
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        let sky = |y| (light[LocalPos::new(16, y, 16).index()] >> 4) & 0x0F;
        println!("shaft sky: y18={} y10={} y1={}", sky(18), sky(10), sky(1));
        assert_eq!(sky(18), 0, "sealed shaft top (2 solid above) must be dark");
        assert_eq!(sky(1), 0, "sealed shaft bottom must be dark");
    }

    #[test]
    fn upward_tunnel_across_chunk_seam_stays_dark() {
        // Model the real case: a LOWER chunk full of stone with a vertical air
        // shaft carved up to its TOP edge (local y=31). The chunk ABOVE has the
        // surface (solid up to some height, air above). The lower chunk's
        // top_sky from the heightmap: its columns are BELOW the surface, so
        // top_sky=0. The only sky that could enter is via the +Y overhead plane
        // (sky[2]) = the upper chunk's -Y face sky. If the upper chunk is solid
        // at the seam above the shaft, that plane is 0 there → shaft stays dark.
        let reg = BlockRegistry::default_set();
        let mut lower = Chunk::filled(STONE);
        // Carve shaft column (16,16) for the full height of the lower chunk.
        for y in 0..CHUNK_SIZE as u8 {
            lower.set(LocalPos::new(16, y, 16), BlockId::AIR);
        }
        // Overhead (+Y) sky plane: the upper chunk is SOLID at the seam above
        // the shaft (surface is higher up), so its bottom sky is 0 there.
        let n = CHUNK_SIZE;
        let above = vec![0u8; n * n]; // all 0: solid seam above
        let sky: NeighborSky = [None, None, Some(above), None, None, None];
        let dark_top = vec![0u8; n * n]; // heightmap: below surface
        let (light, _) =
            compute_chunk_light_2ch(&lower, &reg, &no_borders(), &sky, &dark_top, true);
        let sky_at = |y| (light[LocalPos::new(16, y, 16).index()] >> 4) & 0x0F;
        println!(
            "seam shaft: top31={} mid16={} bot0={}",
            sky_at(31),
            sky_at(16),
            sky_at(0)
        );
        assert_eq!(
            sky_at(31),
            0,
            "shaft top at seam must be dark (solid above)"
        );
        assert_eq!(sky_at(0), 0, "shaft bottom dark");
    }

    #[test]
    fn capped_shaft_below_in_chunk_surface_stays_dark() {
        // Exact repro of the VPROBE case: chunk spans local y=0..31. Solid
        // surface at y=18 (open air y=19..31 above it -> top_sky=15 floods down
        // and stops at 18). A sealed shaft column at (x,z)=(13,3) runs y=1..11,
        // capped by solid y=12..18 (>=6 solid above it). Everything else solid.
        // The shaft MUST be fully dark; top_sky=15 must not leak past the cap.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::filled(STONE);
        let n = CHUNK_SIZE as u8;
        // Open air above the surface everywhere: clear y=19..31.
        for z in 0..n {
            for x in 0..n {
                for y in 19..n {
                    chunk.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        // Carve the sealed shaft at (13,3), y=1..=11.
        for y in 1..=11u8 {
            chunk.set(LocalPos::new(13, y, 3), BlockId::AIR);
        }
        let top = open_sky_top(); // top_sky=15 (column open above this chunk)
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &no_sky(), &top, true);
        let sky = |x, y, z| (light[LocalPos::new(x, y, z).index()] >> 4) & 0x0F;
        println!(
            "surface col: y31={} y19={} y18(solid)={}",
            sky(13, 31, 3),
            sky(13, 19, 3),
            sky(13, 18, 3)
        );
        println!(
            "shaft: y11={} y6={} y1={}",
            sky(13, 11, 3),
            sky(13, 6, 3),
            sky(13, 1, 3)
        );
        assert_eq!(sky(13, 19, 3), 15, "open air above surface lit");
        assert_eq!(sky(13, 11, 3), 0, "sealed shaft top dark");
        assert_eq!(sky(13, 1, 3), 0, "sealed shaft bottom dark");
    }

    #[test]
    fn seam_no_daylight_leak_into_subsurface_shaft() {
        // Two stacked chunks model the real bug. LOWER chunk (origin.y=0):
        // surface solid at local y=18 (h=18, INSIDE the chunk), open air
        // y=19..31 above it, sealed shaft column (13,3) at y=1..11 capped by
        // solid y=12..18. UPPER chunk (origin.y=32): all air.
        //
        // App rule: top_sky=15 only when h < origin.y. For the lower chunk
        // h=18 >= 0, so its top_sky=0; daylight must arrive via the upper
        // chunk's -Y sky plane (the +Y neighbor border) and BFS down to y=18.
        let reg = BlockRegistry::default_set();

        // Upper chunk: all air, top_sky=15 (h=18 < origin.y=32) -> fully lit.
        let upper = Chunk::new_air();
        let (upper_light, _) = compute_chunk_light_2ch(
            &upper,
            &reg,
            &no_borders(),
            &no_sky(),
            &open_sky_top(),
            true,
        );
        // Its -Y face plane (what it hands down to the lower chunk's +Y).
        let n = CHUNK_SIZE;
        let mut upper_bottom = vec![0u8; n * n];
        for z in 0..n {
            for x in 0..n {
                let s = (upper_light[LocalPos::new(x as u8, 0, z as u8).index()] >> 4) & 0x0F;
                upper_bottom[x + z * n] = s;
            }
        }

        // Lower chunk geometry.
        let mut lower = Chunk::filled(STONE);
        for z in 0..n as u8 {
            for x in 0..n as u8 {
                for y in 19..n as u8 {
                    lower.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        for y in 1..=11u8 {
            lower.set(LocalPos::new(13, y, 3), BlockId::AIR);
        }

        // Lower chunk: top_sky=0 (surface inside), overhead plane = upper_bottom.
        let lower_top = vec![0u8; n * n];
        let sky: NeighborSky = [None, None, Some(upper_bottom), None, None, None];
        let (light, _) =
            compute_chunk_light_2ch(&lower, &reg, &no_borders(), &sky, &lower_top, true);
        let sky_at = |x, y, z| (light[LocalPos::new(x, y, z).index()] >> 4) & 0x0F;
        println!(
            "open y19={} surface y18={} shaft y11={} y6={} y1={}",
            sky_at(13, 19, 3),
            sky_at(13, 18, 3),
            sky_at(13, 11, 3),
            sky_at(13, 6, 3),
            sky_at(13, 1, 3)
        );
        assert_eq!(
            sky_at(13, 19, 3),
            15,
            "open air above surface lit from overhead plane"
        );
        assert_eq!(
            sky_at(13, 11, 3),
            0,
            "sealed shaft top dark (no daylight leak)"
        );
        assert_eq!(sky_at(13, 1, 3), 0, "sealed shaft bottom dark");
    }
    // Mirrors vox-app's top_sky_from_heightmap decision for one column so the
    // heightmap->daylight rule is regression-tested headlessly. `h` is the
    // column's highest-solid world-Y; `known` is whether the heightmap has an
    // entry yet (false during streaming before the column's solid chunks load).
    fn top_sky_decision(known: bool, h: i64, origin_y: i64) -> u8 {
        // Unknown column => covered (never invent daylight for a column we have
        // no height for; it self-corrects once terrain loads and relights).
        if !known {
            return 0;
        }
        // Known: full daylight only if the surface is below this whole chunk.
        if h < origin_y { MAX_LIGHT } else { 0 }
    }

    #[test]
    fn topsky_unknown_column_is_covered_known_open_is_lit() {
        let origin_y = -64; // a deep-underground chunk
        // Unknown column (solid chunks not streamed yet) must be DARK. The old
        // rule defaulted missing columns to i64::MIN which is < origin_y and so
        // produced full daylight underground (the sealed-hole leak source).
        assert_eq!(top_sky_decision(false, i64::MIN, origin_y), 0);
        // A genuinely open column (surface far below this chunk) is daylit.
        assert_eq!(top_sky_decision(true, -100, origin_y), MAX_LIGHT);
        // A covered column (surface in/above this chunk) is dark.
        assert_eq!(top_sky_decision(true, 20, origin_y), 0);
        // Surface-in-chunk boundary stays dark at the top face (daylight must
        // arrive via the overhead plane and stop at the surface, not blanket).
        assert_eq!(top_sky_decision(true, origin_y, origin_y), 0);
    }

    #[test]
    fn border_ring_must_not_conduct_skylight_down_to_sealed_cave() {
        // Regression for the sealed-cave daylight leak. One chunk: terrain
        // surface at y=18 (solid 0..=18, open air 19..=31). A SEALED cave at
        // x=0..=1, y=5..=8, z=10..=12 — fully enclosed by solid within this
        // chunk, but touching the -X chunk boundary. The -X neighbor is the
        // same terrain: its +X face is solid (sky 0) below y=19 and open lit
        // air (sky 15) above.
        //
        // Correct behavior: the neighbor's face is SOLID at the cave's depth,
        // so nothing can enter there; the 15s at y>=19 are separated from the
        // cave by real stone in the neighbor. The cave must be DARK.
        //
        // The bug this guards against: the padded border ring was treated as
        // universally non-solid, so a 15 seeded at ring cell (0, ~20, z) rode
        // straight DOWN the ring at full strength (the going-down rule),
        // through what is actually the neighbor's solid stone, and injected
        // 14 sideways into the sealed cave at depth.
        let reg = BlockRegistry::default_set();
        let n = CHUNK_SIZE;
        let mut chunk = Chunk::new_air();
        for z in 0..n {
            for x in 0..n {
                for y in 0..=18usize {
                    chunk.set(LocalPos::new(x as u8, y as u8, z as u8), STONE);
                }
            }
        }
        for z in 10..=12u8 {
            for x in 0..=1u8 {
                for y in 5..=8u8 {
                    chunk.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        // -X neighbor sky plane, indexed (u,v) = (y,z): 15 above the surface,
        // 0 in the solid below — exactly a real terrain neighbor's face.
        let mut west = vec![0u8; n * n];
        for z in 0..n {
            for y in 19..n {
                west[y + z * n] = 15;
            }
        }
        let sky: NeighborSky = [None, Some(west), None, None, None, None];
        let dark_top = vec![0u8; n * n]; // surface inside chunk -> top_sky 0
        let (light, _) =
            compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &sky, &dark_top, true);
        let sky_at = |x, y, z| (light[LocalPos::new(x, y, z).index()] >> 4) & 0x0F;
        assert_eq!(
            sky_at(0, 6, 11),
            0,
            "sealed cave cell on the boundary must be dark"
        );
        assert_eq!(sky_at(1, 6, 11), 0, "sealed cave interior must be dark");
        // Sanity: open air above the surface still receives the side spill.
        assert!(
            sky_at(0, 20, 11) > 0,
            "open air above surface still lit by side plane"
        );
    }

    #[test]
    fn border_ring_must_not_conduct_block_light_around_solid() {
        // Same conduit flaw, block-light flavor. A solid chunk with a sealed
        // pocket at the -X boundary at y=6. The -X neighbor plane carries a
        // lamp's light at y=12 (same z) — separated from the pocket by solid
        // in the neighbor. Without the source-not-conduit rule, the light
        // relays DOWN the border ring (attenuating) and enters the pocket.
        let reg = BlockRegistry::default_set();
        let n = CHUNK_SIZE;
        let mut chunk = Chunk::filled(STONE);
        chunk.set(LocalPos::new(0, 6, 11), BlockId::AIR); // sealed pocket
        let mut west = vec![0u8; n * n];
        west[12 + 11 * n] = 14; // bright block light at (y=12, z=11)
        let borders: NeighborLight = [None, Some(west), None, None, None, None];
        let sky: NeighborSky = [None, None, None, None, None, None];
        let dark_top = vec![0u8; n * n];
        let (light, _) = compute_chunk_light_2ch(&chunk, &reg, &borders, &sky, &dark_top, true);
        let block_at = |x, y, z| light[LocalPos::new(x, y, z).index()] & 0x0F;
        assert_eq!(
            block_at(0, 6, 11),
            0,
            "sealed pocket must not receive relayed block light"
        );
    }

    #[test]
    fn column_heights_uniform_fast_path_matches_scan() {
        let reg = BlockRegistry::default_set();
        let n = CHUNK_SIZE;
        // All-air: every column None.
        let air = Chunk::new_air();
        assert!(chunk_column_heights(&air, &reg).iter().all(|h| h.is_none()));
        // All-solid: every column tops at n-1.
        let solid = Chunk::filled(STONE);
        assert!(
            chunk_column_heights(&solid, &reg)
                .iter()
                .all(|&h| h == Some((n - 1) as u8))
        );
        // Mixed chunk still uses the scan: surface at y=18 everywhere except
        // one column carved down to y=3.
        let mut mixed = Chunk::filled(STONE);
        for y in 19..n as u8 {
            for z in 0..n as u8 {
                for x in 0..n as u8 {
                    mixed.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        for y in 4..=18u8 {
            mixed.set(LocalPos::new(5, y, 5), BlockId::AIR);
        }
        let h = chunk_column_heights(&mixed, &reg);
        assert_eq!(h[column_index(4, 5)], Some(18));
        assert_eq!(h[column_index(5, 5)], Some(3));
    }

    #[test]
    fn border_changed_faces_reports_only_the_changed_face() {
        let reg = BlockRegistry::default_set();
        let chunk = Chunk::filled(STONE); // stored light: all zero
        // A fresh light buffer identical to stored (all zero) → no faces.
        let same = vec![0u8; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
        assert_eq!(packed_border_changed_faces(&chunk, &same), 0);
        let _ = &reg;
        // Change one cell on the -Z face (z = 0) only → exactly bit 5.
        let mut z0 = same.clone();
        z0[LocalPos::new(7, 9, 0).index()] = 0x30; // sky=3
        assert_eq!(packed_border_changed_faces(&chunk, &z0), 1 << 5);
        // Change a corner cell (0,0,0): lies on -X, -Y, and -Z faces.
        let mut corner = same;
        corner[LocalPos::new(0, 0, 0).index()] = 0x10;
        assert_eq!(
            packed_border_changed_faces(&chunk, &corner),
            (1 << 1) | (1 << 3) | (1 << 5)
        );
    }

    /// Perf yardstick, not a correctness test. Run with:
    /// `cargo test --release -p vox-core bench_relight -- --ignored --nocapture`
    /// History: 1438us (tuple-queue BFS) -> 1209us (solidity map)
    ///          -> 447us (bucketed flood + column pre-fill), July 2026.
    #[test]
    #[ignore]
    fn bench_relight_realistic_surface_chunk() {
        let reg = BlockRegistry::default_set();
        let n = CHUNK_SIZE;
        let mut chunk = Chunk::new_air();
        for z in 0..n {
            for x in 0..n {
                for y in 0..=18usize {
                    chunk.set(LocalPos::new(x as u8, y as u8, z as u8), STONE);
                }
            }
        }
        for z in 8..=14u8 {
            for x in 8..=14u8 {
                for y in 4..=9u8 {
                    chunk.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        for y in 2..=31u8 {
            chunk.set(LocalPos::new(20, y, 20), BlockId::AIR);
        }
        let mut side = vec![0u8; n * n];
        for z in 0..n {
            for y in 19..n {
                side[y + z * n] = 15;
            }
        }
        let overhead = vec![15u8; n * n];
        let sky: NeighborSky = [
            Some(side.clone()),
            Some(side.clone()),
            Some(overhead),
            None,
            Some(side.clone()),
            Some(side),
        ];
        let mut top = vec![0u8; n * n];
        top[20 + 20 * n] = 15;
        let iters = 300;
        let start = std::time::Instant::now();
        let mut sink = 0u64;
        for _ in 0..iters {
            let (light, _) = compute_chunk_light_2ch(&chunk, &reg, &no_borders(), &sky, &top, true);
            sink = sink.wrapping_add(light[12345] as u64);
        }
        let total = start.elapsed();
        println!(
            "relight x{iters}: total {:?}, per-chunk {:.0}us (sink {sink})",
            total,
            total.as_secs_f64() * 1e6 / iters as f64
        );
    }
}
