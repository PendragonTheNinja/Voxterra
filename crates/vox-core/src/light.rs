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
            if vol.is_solid(nx, ny, nz) {
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
            if vol.is_solid(nx, ny, nz) {
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
            if vol.is_solid(nx, ny, nz) {
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
struct PaddedChunk<'a> {
    chunk: &'a Chunk,
    registry: &'a BlockRegistry,
    emission: Vec<u8>, // 34³, border = neighbor light, center = block emission
    light: Vec<u8>,    // 34³ scratch
}

const PAD: usize = CHUNK_SIZE + 2;

impl<'a> PaddedChunk<'a> {
    fn idx(x: usize, y: usize, z: usize) -> usize {
        x + y * PAD + z * PAD * PAD
    }

    fn build(chunk: &'a Chunk, registry: &'a BlockRegistry, borders: &NeighborLight) -> Self {
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
            chunk,
            registry,
            emission,
            light: vec![0u8; PAD * PAD * PAD],
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

impl LightVolume for PaddedChunk<'_> {
    fn dims(&self) -> [usize; 3] {
        [PAD, PAD, PAD]
    }
    fn is_solid(&self, x: usize, y: usize, z: usize) -> bool {
        // Border cells are non-solid (they only feed light in). Center cells
        // use the registry's solidity for the block.
        match Self::center_local(x, y, z) {
            Some(pos) => self.registry.is_solid(self.chunk.get(pos)),
            None => false,
        }
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
    compute_chunk_light_2ch(chunk, registry, borders, &NEIGHBOR_SKY_NONE, true)
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
    do_sky: bool,
) -> (Vec<u8>, bool) {
    // --- Block light (low nibble) ---
    let block = {
        let mut vol = PaddedChunk::build(chunk, registry, block_borders);
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
        compute_padded_sky(chunk, registry, sky_borders)
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
fn compute_padded_sky(chunk: &Chunk, registry: &BlockRegistry, sky: &NeighborSky) -> Vec<u8> {
    let n = CHUNK_SIZE;
    let mut light = vec![0u8; PAD * PAD * PAD];

    // Solidity lookup for a padded coord (center from blocks; border non-solid).
    let solid = |x: usize, y: usize, z: usize| -> bool {
        match PaddedChunk::center_local(x, y, z) {
            Some(pos) => registry.is_solid(chunk.get(pos)),
            None => false,
        }
    };

    // Seed border ring from neighbor sky planes. Face order 0:+X 1:-X 2:+Y
    // 3:-Y 4:+Z 5:-Z. The +Y face (index 2) defaults to 15 (open sky) where
    // no plane is given.
    let mut queue: Vec<(usize, usize, usize)> = Vec::new();
    let seed = |light: &mut [u8], queue: &mut Vec<(usize, usize, usize)>, x, y, z, val: u8| {
        if val > 0 {
            light[PaddedChunk::idx(x, y, z)] = val;
            queue.push((x, y, z));
        }
    };

    for v in 0..n {
        for u in 0..n {
            // +X (x = n+1), plane (y,z) = (u,v)
            if let Some(p) = &sky[0] {
                seed(&mut light, &mut queue, n + 1, u + 1, v + 1, p[u + v * n]);
            }
            // -X (x = 0)
            if let Some(p) = &sky[1] {
                seed(&mut light, &mut queue, 0, u + 1, v + 1, p[u + v * n]);
            }
            // +Y (y = n+1), plane (x,z) = (u,v) — open-sky default 15
            let top_val = match &sky[2] {
                Some(p) => p[u + v * n],
                None => MAX_LIGHT,
            };
            seed(&mut light, &mut queue, u + 1, n + 1, v + 1, top_val);
            // -Y (y = 0)
            if let Some(p) = &sky[3] {
                seed(&mut light, &mut queue, u + 1, 0, v + 1, p[u + v * n]);
            }
            // +Z (z = n+1), plane (x,y) = (u,v)
            if let Some(p) = &sky[4] {
                seed(&mut light, &mut queue, u + 1, v + 1, n + 1, p[u + v * n]);
            }
            // -Z (z = 0)
            if let Some(p) = &sky[5] {
                seed(&mut light, &mut queue, u + 1, v + 1, 0, p[u + v * n]);
            }
        }
    }

    // BFS with the sky rule (no attenuation straight down at full strength).
    let mut head = 0;
    while head < queue.len() {
        let (x, y, z) = queue[head];
        head += 1;
        let level = light[PaddedChunk::idx(x, y, z)];
        if level <= 1 {
            continue;
        }
        for (nx, ny, nz) in neighbors(x, y, z, PAD, PAD, PAD) {
            if solid(nx, ny, nz) {
                continue;
            }
            let going_down = nx == x && nz == z && ny + 1 == y;
            let spread = if going_down && level == MAX_LIGHT {
                MAX_LIGHT
            } else {
                level - 1
            };
            let ni = PaddedChunk::idx(nx, ny, nz);
            if light[ni] < spread {
                light[ni] = spread;
                queue.push((nx, ny, nz));
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
    let (light, changed) =
        compute_chunk_light_2ch(chunk, registry, borders, &NEIGHBOR_SKY_NONE, false);
    apply_chunk_light(chunk, &light);
    changed
}

/// Recompute **both** light channels for a chunk in place (compute + apply).
/// Convenience wrapper over [`compute_chunk_light_2ch`] for tests.
pub fn relight_chunk_2ch(
    chunk: &mut Chunk,
    registry: &BlockRegistry,
    block_borders: &NeighborLight,
    sky_borders: &NeighborSky,
) -> bool {
    let (light, changed) =
        compute_chunk_light_2ch(chunk, registry, block_borders, sky_borders, true);
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
        // No solid blocks; supply a +Y neighbor whose bottom plane is full
        // daylight (an open shaft coming from above) → sky floods down to 15.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let plane = vec![15u8; n * n]; // neighbor-above bottom row all lit
        let sky: NeighborSky = [None, None, Some(plane), None, None, None];
        relight_chunk_2ch(&mut chunk, &reg, &no_borders(), &sky);

        // Top cell receives 15 (down rule across boundary), and stays 15 down.
        assert_eq!(chunk.sky_light(LocalPos::new(16, 31, 16)), 15);
        assert_eq!(chunk.sky_light(LocalPos::new(16, 0, 16)), 15);
    }

    #[test]
    fn two_channel_dark_neighbor_above_blocks_sky() {
        // A +Y neighbor whose bottom plane is dark (it has a roof) → no sky
        // enters from above; with no other source, the chunk is sky-dark.
        let reg = BlockRegistry::default_set();
        let mut chunk = Chunk::new_air();
        let n = CHUNK_SIZE;
        let plane = vec![0u8; n * n];
        let sky: NeighborSky = [None, None, Some(plane), None, None, None];
        relight_chunk_2ch(&mut chunk, &reg, &no_borders(), &sky);

        assert_eq!(
            chunk.sky_light(LocalPos::new(16, 31, 16)),
            0,
            "dark neighbor above => shadow (no open-sky default when plane given)"
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
}
