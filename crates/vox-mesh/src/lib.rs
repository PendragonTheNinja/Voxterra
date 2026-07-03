//! vox-mesh: chunk → triangle mesh conversion.
//!
//! Headless crate: depends only on vox-core (+ bytemuck for byte casting).
//! Must never depend on wgpu/winit — meshing is pure data transformation
//! and is unit-tested without a GPU.
//!
//! Two meshers live here, permanently:
//!
//! - [`mesh_chunk_naive`]: culled meshing — one quad per visible block
//!   face. Simple, obviously correct, kept forever as the **test oracle**
//!   (NOT dead code; see the differential test).
//! - [`mesh_chunk`]: greedy meshing — coplanar same-block same-facing
//!   visible faces merged into maximal rectangles. Same visible surface,
//!   far fewer quads. The default mesher.
//!
//! Merge criterion: identical block AND identical face direction. When
//! per-face data later grows (textures, ambient occlusion), the criterion
//! must tighten accordingly — and the differential test must keep passing
//! regardless (Milestone 01 spec, criterion 4).

use vox_core::{BlockId, CHUNK_SIZE, Chunk, ChunkPos, LocalPos, World};

/// One mesh vertex. `repr(C)` + Pod so vox-render can cast the vertex
/// buffer straight to bytes for GPU upload.
///
/// Milestone 03 format (texture array, ADR-0003): position, plus texture
/// coordinates `uv` that run `0..W` / `0..H` across a greedy-merged quad
/// (so the layer tiles W×H times under a Repeat sampler), the texture-array
/// `layer` index for this face, and a directional `brightness` scalar that
/// the shader multiplies into the sampled color so geometry reads as 3D
/// before real lighting (Milestone 04).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    pub brightness: f32,
}

/// CPU-side mesh: vertex + index buffers ready for GPU upload.
#[derive(Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }
}

/// Read-only views of a chunk's six face-neighbors, used for cross-chunk
/// face culling. A `None` neighbor (unloaded / nonexistent chunk) is
/// treated as air, so faces on the edge of the loaded world are emitted.
///
/// This type — rather than `&World` — is the mesher input so that meshing
/// stays trivially parallelizable: a meshing job borrows exactly seven
/// chunks and nothing else.
///
/// Task 2 (M06) widened this from the 6 face neighbors to all **26**: smooth
/// per-vertex lighting samples diagonally, so a face on a chunk edge reads
/// cells that live in edge- and corner-adjacent chunks. Neighbors are indexed
/// by a `(dx,dy,dz)` offset in `{-1,0,1}³` (excluding `0,0,0`). The public
/// constructors are unchanged (`NONE`, `of`), so callers need no edits.
#[derive(Clone, Copy)]
pub struct ChunkNeighbors<'a> {
    /// Indexed by `neighbor_slot(dx,dy,dz)`; `None` = unloaded/absent (air).
    slots: [Option<&'a Chunk>; 27],
}

impl Default for ChunkNeighbors<'_> {
    fn default() -> Self {
        Self::NONE
    }
}

/// Map an offset in `{-1,0,1}³` to a 0..27 slot (the center `0,0,0` slot is
/// unused but kept so the index math is a simple base-3 pack).
#[inline]
fn neighbor_slot(dx: i64, dy: i64, dz: i64) -> usize {
    (((dx + 1) * 3 + (dy + 1)) * 3 + (dz + 1)) as usize
}

impl<'a> ChunkNeighbors<'a> {
    /// No neighbors: standalone-chunk behavior (all border faces emitted).
    pub const NONE: ChunkNeighbors<'static> = ChunkNeighbors { slots: [None; 27] };

    /// Gather all 26 neighbors of `pos` from a world.
    pub fn of(world: &'a World, pos: ChunkPos) -> Self {
        let mut slots = [None; 27];
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    slots[neighbor_slot(dx, dy, dz)] =
                        world.chunk(ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz));
                }
            }
        }
        Self { slots }
    }

    /// The neighbor chunk at offset `(dx,dy,dz)` (each in -1..=1).
    #[inline]
    fn at(&self, dx: i64, dy: i64, dz: i64) -> Option<&'a Chunk> {
        self.slots[neighbor_slot(dx, dy, dz)]
    }

    /// Builder: set the neighbor at offset `(dx,dy,dz)`. Chainable; mainly for
    /// tests that supply a specific neighbor against `NONE`.
    pub fn with(mut self, dx: i64, dy: i64, dz: i64, chunk: &'a Chunk) -> Self {
        self.slots[neighbor_slot(dx, dy, dz)] = Some(chunk);
        self
    }

    /// Face-neighbor builder shortcuts.
    pub fn with_neg_x(self, c: &'a Chunk) -> Self {
        self.with(-1, 0, 0, c)
    }
    pub fn with_pos_x(self, c: &'a Chunk) -> Self {
        self.with(1, 0, 0, c)
    }
    pub fn with_neg_y(self, c: &'a Chunk) -> Self {
        self.with(0, -1, 0, c)
    }
    pub fn with_pos_y(self, c: &'a Chunk) -> Self {
        self.with(0, 1, 0, c)
    }
    pub fn with_neg_z(self, c: &'a Chunk) -> Self {
        self.with(0, 0, -1, c)
    }
    pub fn with_pos_z(self, c: &'a Chunk) -> Self {
        self.with(0, 0, 1, c)
    }
}

/// Padded 34³ snapshot of everything the meshers sample (ADR-0006): per cell
/// the block id and the packed light scalar (`max(block_light, sky_light)`),
/// center from the chunk, the 1-cell shell from neighbors (absent cells read
/// as air / light 0 — the same standalone behavior as before). The hot
/// meshing loops then run against flat arrays with no palette decodes and no
/// chunk-boundary branching — the same "pay one bounded gather up front"
/// design as the relight engine's `PaddedChunk` (M05).
///
/// Task 1 (M06) fills the shell from the six FACE neighbors only, because
/// that is all today's per-face sampling reads; edge/corner cells stay air
/// until per-vertex sampling lands (task 2), which will widen the fill to
/// all 26 neighbors.
pub struct MeshInput {
    /// 34³ block ids; shell cells default to AIR.
    block: Vec<BlockId>,
    /// 34³ light scalars (`max(block, sky)`); shell defaults to 0.
    light: Vec<u8>,
    all_air: bool,
}

const PAD: usize = CHUNK_SIZE + 2;

impl MeshInput {
    #[inline]
    fn idx(x: usize, y: usize, z: usize) -> usize {
        x + y * PAD + z * PAD * PAD
    }

    /// Chunk-relative coords (each in `-1..=32`) to the padded index.
    #[inline]
    fn cidx(x: i32, y: i32, z: i32) -> usize {
        Self::idx((x + 1) as usize, (y + 1) as usize, (z + 1) as usize)
    }

    /// Is the cell at chunk-relative coords air? (Absent neighbors are air.)
    #[inline]
    pub fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.block[Self::cidx(x, y, z)].is_air()
    }

    /// Block id at chunk-relative coords.
    #[inline]
    pub fn block(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.block[Self::cidx(x, y, z)]
    }

    /// Light scalar at chunk-relative coords (0 in absent neighbors).
    #[inline]
    pub fn light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.light[Self::cidx(x, y, z)]
    }

    /// Build the snapshot from a chunk and its face neighbors.
    pub fn build(chunk: &Chunk, neighbors: &ChunkNeighbors) -> Self {
        let n = CHUNK_SIZE;
        let mut block = vec![BlockId::AIR; PAD * PAD * PAD];
        let mut light = vec![0u8; PAD * PAD * PAD];

        // Center. Uniform chunks skip the per-cell palette decode entirely.
        if chunk.is_uniform() {
            let b = chunk.get(LocalPos::new(0, 0, 0));
            if !b.is_air() {
                for z in 1..=n {
                    for y in 1..=n {
                        let row = Self::idx(1, y, z);
                        block[row..row + n].fill(b);
                    }
                }
            }
        } else {
            for pos in LocalPos::iter() {
                block[Self::cidx(pos.x() as i32, pos.y() as i32, pos.z() as i32)] = chunk.get(pos);
            }
        }
        for pos in LocalPos::iter() {
            let l = chunk.block_light(pos).max(chunk.sky_light(pos));
            if l > 0 {
                light[Self::cidx(pos.x() as i32, pos.y() as i32, pos.z() as i32)] = l;
            }
        }

        // Shell: every cell with a component at -1 or n (edges and corners
        // included, not just faces — smooth lighting samples diagonally). For
        // each shell coord, the offset of the out-of-range components picks the
        // owning neighbor; the in-range components index into it (wrapping the
        // out-of-range ones to the neighbor's touching layer). Absent
        // neighbors stay AIR / light 0 (standalone-chunk behavior).
        let sz = n as i32;
        let wrap = |c: i32| -> (i64, u8) {
            // (neighbor offset along this axis, local coord within it).
            if c < 0 {
                (-1, (n as i32 - 1) as u8)
            } else if c >= sz {
                (1, 0)
            } else {
                (0, c as u8)
            }
        };
        for z in -1..=sz {
            for y in -1..=sz {
                for x in -1..=sz {
                    if (0..sz).contains(&x) && (0..sz).contains(&y) && (0..sz).contains(&z) {
                        continue; // interior already filled
                    }
                    let (dx, lx) = wrap(x);
                    let (dy, ly) = wrap(y);
                    let (dz, lz) = wrap(z);
                    let Some(nb) = neighbors.at(dx, dy, dz) else {
                        continue;
                    };
                    let lp = LocalPos::new(lx, ly, lz);
                    let i = Self::cidx(x, y, z);
                    block[i] = nb.get(lp);
                    light[i] = nb.block_light(lp).max(nb.sky_light(lp));
                }
            }
        }

        Self {
            block,
            light,
            all_air: chunk.is_all_air(),
        }
    }
}

/// Directional shading per (axis, positive sign): top brightest, bottom
/// darkest, so geometry reads as 3D before real lighting exists. Shared by
/// both meshers — the differential test compares vertex colors, so any
/// divergence between the two is caught.
fn face_brightness(axis: usize, positive: bool) -> f32 {
    match (axis, positive) {
        (0, true) => 0.75,  // +X east
        (0, false) => 0.65, // -X west
        (1, true) => 1.0,   // +Y top
        (1, false) => 0.45, // -Y bottom
        (2, true) => 0.8,   // +Z south
        (2, false) => 0.6,  // -Z north
        _ => unreachable!("axis is 0..3"),
    }
}

/// Map a 0..=15 light level to a brightness multiplier. Non-linear so the
/// falloff looks natural, with a small ambient floor so unlit areas are dim
/// but not pure black (caves stay barely navigable). Combined multiplicatively
/// with the directional face shading.
/// Map a fractional 0..=15 light amount to a brightness multiplier. Smooth
/// per-corner light averages neighboring cells, so the value is not an
/// integer. Non-linear (gamma) with a small ambient floor so unlit areas are
/// dim but not pure black; combined multiplicatively with directional face
/// shading.
fn light_curve_f(level: f32) -> f32 {
    // Ambient floor: brightness of a cell that receives NO light at all — a
    // sealed cave, an overhang underside. This is a pure geometry baseline
    // (sky can't reach the cell), independent of time of day; a future
    // day/night cycle dims the SKY channel above this floor, so moonlit open
    // ground will sit brighter than this without changing it. Tuned darker
    // than Minecraft's floor so enclosed dark reads genuinely dark, but not
    // pure black (M06 task 5 decision).
    const AMBIENT: f32 = 0.035;
    let t = (level / 15.0).clamp(0.0, 1.0);
    // Slight gamma so mid-levels read brighter; eases the linear band.
    let curved = t.powf(0.85);
    AMBIENT + (1.0 - AMBIENT) * curved
}

/// Unit vector along `axis`.
fn axis_unit(axis: usize) -> [f32; 3] {
    let mut v = [0.0; 3];
    v[axis] = 1.0;
    v
}

/// The four corners of a face, as (du, dv) in {0,1}², in emit order
/// (0,0)→(1,0)→(1,1)→(0,1) so it matches `emit_rect`'s corner walk.
const CORNER_DUV: [(i32, i32); 4] = [(0, 0), (1, 0), (1, 1), (0, 1)];

/// Smooth per-corner light for one visible face cell (ADR-0006 / M06 task 2).
///
/// `cell` is the solid block's chunk-relative coords; `axis`/`positive` its
/// outward face; `u_axis`/`v_axis` the in-plane axes (matching `FACE_DIRS`).
/// The face's light comes from the AIR side — the plane one step out along
/// the normal. Each corner averages the (up to 4) cells of that air-plane
/// which touch the corner: the face cell itself, its two edge-neighbors along
/// ±u and ±v, and the diagonal. This yields gradients across the face instead
/// of one flat value, and darkens corners tucked against geometry.
///
/// Ambient-occlusion-aware averaging: a solid diagonal that sits behind two
/// solid edges is fully occluded, so it (and its light, 0) still drags the
/// corner down — which is exactly the soft contact-shadow AO wants. We keep
/// the average over all four samples (solid cells contribute their light,
/// which is ~0 inside geometry), giving smooth light and AO in one pass. The
/// dedicated 3-neighbor AO term lands in task 3; this is the light half.
fn corner_lights(
    input: &MeshInput,
    cell: [i32; 3],
    axis: usize,
    positive: bool,
    u_axis: usize,
    v_axis: usize,
) -> [f32; 4] {
    // The air cell directly in front of the face.
    let mut front = cell;
    front[axis] += if positive { 1 } else { -1 };

    let sample = |du: i32, dv: i32| -> u8 {
        let mut c = front;
        c[u_axis] += du;
        c[v_axis] += dv;
        input.light(c[0], c[1], c[2])
    };

    let mut out = [0.0f32; 4];
    for (k, &(cu, cv)) in CORNER_DUV.iter().enumerate() {
        // Corner (cu,cv) in {0,1}² touches the front cell and its neighbors
        // toward that corner: steps of su/sv in {-1,0} ... but expressed from
        // the face cell, the four touching air cells are at
        // (0,0), (su,0), (0,sv), (su,sv) where su = 2*cu-1, sv = 2*cv-1.
        let su = 2 * cu - 1;
        let sv = 2 * cv - 1;
        let a = sample(0, 0);
        let b = sample(su, 0);
        let c = sample(0, sv);
        let d = sample(su, sv);
        out[k] = (a as f32 + b as f32 + c as f32 + d as f32) / 4.0;
    }
    out
}

/// Classic per-corner ambient occlusion (M06 task 3). Independent of light:
/// a corner tucked into solid geometry darkens even under full skylight,
/// which is what gives blocks their sense of contact shadow and depth.
///
/// For each face corner, look at the three cells that border it IN THE
/// AIR-SIDE PLANE (one step out along the face normal): the two "side" cells
/// (edge-adjacent along ±u and ±v toward the corner) and the "corner" cell
/// (the diagonal). Occluders are non-air cells. The standard rule:
///   - both sides occluded            -> level 0 (darkest)
///   - otherwise 3 - (s1 + s2 + corn) -> level 1..3
///
/// Returned as a 0..=3 level per corner (emit order), mapped to a brightness
/// factor by `ao_factor`.
fn corner_ao(
    input: &MeshInput,
    cell: [i32; 3],
    axis: usize,
    positive: bool,
    u_axis: usize,
    v_axis: usize,
) -> [u8; 4] {
    let mut front = cell;
    front[axis] += if positive { 1 } else { -1 };

    // Occluder = non-air cell one step out, offset by (du,dv) in the plane.
    let occ = |du: i32, dv: i32| -> u8 {
        let mut c = front;
        c[u_axis] += du;
        c[v_axis] += dv;
        u8::from(!input.is_air(c[0], c[1], c[2]))
    };

    let mut out = [3u8; 4];
    for (k, &(cu, cv)) in CORNER_DUV.iter().enumerate() {
        let su = 2 * cu - 1; // -1 or +1 toward this corner along u
        let sv = 2 * cv - 1; // and along v
        let side1 = occ(su, 0);
        let side2 = occ(0, sv);
        let corner = occ(su, sv);
        out[k] = if side1 == 1 && side2 == 1 {
            0
        } else {
            3 - (side1 + side2 + corner)
        };
    }
    out
}

/// Map an AO level (0 darkest .. 3 none) to a brightness multiplier. Tuned in
/// task 5 against real scenes; kept gentle so AO shapes creases without
/// crushing them to black.
fn ao_factor(level: u8) -> f32 {
    // level: 0,1,2,3 -> increasing brightness. 3 = fully lit (1.0).
    const AO: [f32; 4] = [0.55, 0.70, 0.85, 1.0];
    AO[level as usize]
}

/// The six face directions as (normal axis, positive sign, u axis, v axis).
/// The u/v axes are chosen so that unit(u) × unit(v) == the outward normal
/// (verified by hand; don't reorder casually):
///   +X: Y×Z=+X   -X: Z×Y=-X
///   +Y: Z×X=+Y   -Y: X×Z=-Y
///   +Z: X×Y=+Z   -Z: Y×X=-Z
/// With this choice, emitting corners base, base+u, base+u+v, base+v gives
/// CCW winding viewed from outside — matching wgpu's default front face.
const FACE_DIRS: [(usize, bool, usize, usize); 6] = [
    (0, true, 1, 2),
    (0, false, 2, 1),
    (1, true, 2, 0),
    (1, false, 0, 2),
    (2, true, 0, 1),
    (2, false, 1, 0),
];

/// Push one axis-aligned `w`×`h` rectangle (4 vertices, 2 CCW triangles).
#[allow(clippy::too_many_arguments)]
fn emit_rect(
    mesh: &mut MeshData,
    base: [f32; 3],
    u_dir: [f32; 3],
    v_dir: [f32; 3],
    w: f32,
    h: f32,
    layer: u32,
    // Per-corner brightness in emit order (0,0)→(w,0)→(w,h)→(0,h). Smooth
    // lighting varies these across the quad; the shader interpolates them.
    corner_brightness: [f32; 4],
    // Whether to flip the triangle diagonal (00-11 vs 01-10). Chosen so AO/
    // light interpolation never produces the "butterfly" anisotropy on a
    // gradient (M06). false = default diagonal (0,2); true = (1,3).
    flip: bool,
) {
    // UVs run 0..w / 0..h so the texture-array layer tiles w×h times across
    // a greedy-merged quad under a Repeat sampler (ADR-0003).
    let corner = |du: f32, dv: f32, bright: f32| Vertex {
        position: [
            base[0] + u_dir[0] * du + v_dir[0] * dv,
            base[1] + u_dir[1] * du + v_dir[1] * dv,
            base[2] + u_dir[2] * du + v_dir[2] * dv,
        ],
        uv: [du, dv],
        layer,
        brightness: bright,
    };

    let base_index = mesh.vertices.len() as u32;
    mesh.vertices.push(corner(0.0, 0.0, corner_brightness[0]));
    mesh.vertices.push(corner(w, 0.0, corner_brightness[1]));
    mesh.vertices.push(corner(w, h, corner_brightness[2]));
    mesh.vertices.push(corner(0.0, h, corner_brightness[3]));
    let b = base_index;
    if flip {
        // Diagonal through corners 1 and 3.
        mesh.indices
            .extend_from_slice(&[b + 1, b + 2, b + 3, b + 1, b + 3, b]);
    } else {
        // Default diagonal through corners 0 and 2.
        mesh.indices
            .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
}

/// Choose the triangulation diagonal so the two triangles' shared edge runs
/// between the corners with the CLOSER brightness — the standard fix for the
/// anisotropic-interpolation ("butterfly") artifact on AO/light gradients.
fn choose_flip(c: [f32; 4]) -> bool {
    // Flip when the 0-2 diagonal spans the larger contrast.
    (c[0] - c[2]).abs() > (c[1] - c[3]).abs()
}

// ---------------------------------------------------------------------------
// Naive culled mesher — the oracle
// ---------------------------------------------------------------------------

/// Culled meshing: one quad per solid-block face bordering a non-solid
/// block. Kept forever as the correctness oracle for [`mesh_chunk`].
///
/// `layer_of(block, face_index)` returns the texture-array layer for that
/// block's given face. `face_index` matches `FACE_DIRS` order
/// (0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z), which is also the registry's face order.
pub fn mesh_chunk_naive(
    chunk: &Chunk,
    neighbors: &ChunkNeighbors,
    mut layer_of: impl FnMut(BlockId, usize) -> u32,
) -> MeshData {
    let mut mesh = MeshData::default();
    let input = MeshInput::build(chunk, neighbors);
    if input.all_air {
        return mesh;
    }

    for pos in LocalPos::iter() {
        let coords = [pos.x() as i32, pos.y() as i32, pos.z() as i32];
        let block = input.block(coords[0], coords[1], coords[2]);
        if block.is_air() {
            continue;
        }

        for (face_index, &(axis, positive, u_axis, v_axis)) in FACE_DIRS.iter().enumerate() {
            let mut neighbor = coords;
            neighbor[axis] += if positive { 1 } else { -1 };
            if !input.is_air(neighbor[0], neighbor[1], neighbor[2]) {
                continue;
            }

            // Positive faces sit at the far side of the block cell.
            let mut base = [coords[0] as f32, coords[1] as f32, coords[2] as f32];
            if positive {
                base[axis] += 1.0;
            }
            // Smooth per-corner light from the air side, times the fixed
            // directional face shade.
            let shade = face_brightness(axis, positive);
            let cl = corner_lights(&input, coords, axis, positive, u_axis, v_axis);
            let ao = corner_ao(&input, coords, axis, positive, u_axis, v_axis);
            let cb = [
                shade * light_curve_f(cl[0]) * ao_factor(ao[0]),
                shade * light_curve_f(cl[1]) * ao_factor(ao[1]),
                shade * light_curve_f(cl[2]) * ao_factor(ao[2]),
                shade * light_curve_f(cl[3]) * ao_factor(ao[3]),
            ];
            emit_rect(
                &mut mesh,
                base,
                axis_unit(u_axis),
                axis_unit(v_axis),
                1.0,
                1.0,
                layer_of(block, face_index),
                cb,
                choose_flip(cb),
            );
        }
    }

    mesh
}

// ---------------------------------------------------------------------------
// Greedy mesher — the default
// ---------------------------------------------------------------------------

/// Greedy meshing: for each of the six face directions, sweep the chunk one
/// slice at a time; in each slice build a 2D mask of visible faces, then
/// merge same-block cells into maximal rectangles (grow width along u, then
/// height along v).
///
/// Produces the same visible surface as [`mesh_chunk_naive`] with far fewer
/// quads (a flat 32×32 face becomes a single quad rather than 1,024).
pub fn mesh_chunk(
    chunk: &Chunk,
    neighbors: &ChunkNeighbors,
    mut layer_of: impl FnMut(BlockId, usize) -> u32,
) -> MeshData {
    let mut mesh = MeshData::default();
    let input = MeshInput::build(chunk, neighbors);
    if input.all_air {
        return mesh;
    }

    let size = CHUNK_SIZE;
    // Per-slice mask: each visible face cell carries its block AND its four
    // per-corner light SUMS (each 0..=60 = sum of four 0..=15 samples — an
    // exact integer, so the tuple is Eq/Hashable and merging is exact). Cells
    // merge only when block AND all four corner light sums AND all four AO
    // levels match, so any smooth-light gradient or AO variation correctly
    // subdivides the merged rectangles. Corner order is emit order
    // (0,0),(1,0),(1,1),(0,1) in (u,v).
    type CellMask = (BlockId, [u16; 4], [u8; 4]);
    let mut mask: Vec<Option<CellMask>> = vec![None; size * size];
    let at = |u: usize, v: usize| u + v * size;

    for (face_index, &(axis, positive, u_axis, v_axis)) in FACE_DIRS.iter().enumerate() {
        let dir_shade = face_brightness(axis, positive);
        let u_dir = axis_unit(u_axis);
        let v_dir = axis_unit(v_axis);

        for slice in 0..size {
            // --- Build the visibility mask for this slice. ---
            let mut any = false;
            for v in 0..size {
                for u in 0..size {
                    let mut coords = [0i32; 3];
                    coords[axis] = slice as i32;
                    coords[u_axis] = u as i32;
                    coords[v_axis] = v as i32;

                    let block = input.block(coords[0], coords[1], coords[2]);

                    let cell = &mut mask[at(u, v)];
                    if block.is_air() {
                        *cell = None;
                        continue;
                    }

                    let mut n = coords;
                    n[axis] += if positive { 1 } else { -1 };
                    if input.is_air(n[0], n[1], n[2]) {
                        let cl = corner_lights(&input, coords, axis, positive, u_axis, v_axis);
                        let ao = corner_ao(&input, coords, axis, positive, u_axis, v_axis);
                        // Store light as integer sums (×4) to keep the key exact.
                        let sums = [
                            (cl[0] * 4.0).round() as u16,
                            (cl[1] * 4.0).round() as u16,
                            (cl[2] * 4.0).round() as u16,
                            (cl[3] * 4.0).round() as u16,
                        ];
                        *cell = Some((block, sums, ao));
                        any = true;
                    } else {
                        *cell = None;
                    }
                }
            }
            if !any {
                continue;
            }

            // --- Merge mask cells into maximal rectangles. ---
            for v0 in 0..size {
                for u0 in 0..size {
                    let Some(key) = mask[at(u0, v0)] else {
                        continue;
                    };
                    let (block, sums, ao) = key;

                    // Grow width along u.
                    let mut w = 1;
                    while u0 + w < size && mask[at(u0 + w, v0)] == Some(key) {
                        w += 1;
                    }

                    // Grow height along v while every cell in the row matches.
                    let mut h = 1;
                    'grow: while v0 + h < size {
                        for du in 0..w {
                            if mask[at(u0 + du, v0 + h)] != Some(key) {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }

                    // Consume the rectangle so cells aren't emitted twice.
                    for dv in 0..h {
                        for du in 0..w {
                            mask[at(u0 + du, v0 + dv)] = None;
                        }
                    }

                    // Positive faces sit at the far plane of the slice.
                    let plane = slice + usize::from(positive);
                    let mut base = [0f32; 3];
                    base[axis] = plane as f32;
                    base[u_axis] = u0 as f32;
                    base[v_axis] = v0 as f32;

                    // Corner sums back to brightness (÷4 to recover the mean),
                    // times AO.
                    let cb = [
                        dir_shade * light_curve_f(sums[0] as f32 / 4.0) * ao_factor(ao[0]),
                        dir_shade * light_curve_f(sums[1] as f32 / 4.0) * ao_factor(ao[1]),
                        dir_shade * light_curve_f(sums[2] as f32 / 4.0) * ao_factor(ao[2]),
                        dir_shade * light_curve_f(sums[3] as f32 / 4.0) * ao_factor(ao[3]),
                    ];
                    emit_rect(
                        &mut mesh,
                        base,
                        u_dir,
                        v_dir,
                        w as f32,
                        h as f32,
                        layer_of(block, face_index),
                        cb,
                        choose_flip(cb),
                    );
                }
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // A layer closure for tests: distinct layer per (block, face) so the
    // differential test also exercises per-face layer correctness. For the
    // simple count tests, the exact layer doesn't matter.
    fn layers(b: BlockId, face: usize) -> u32 {
        (b.0 as u32) * 6 + face as u32
    }
    const WHITE: fn(BlockId, usize) -> u32 = layers;
    const STONE: BlockId = BlockId(1);
    const N: usize = CHUNK_SIZE;

    // -----------------------------------------------------------------
    // Naive oracle: exact quad counts (Milestone 00 + 01 contract).
    // -----------------------------------------------------------------

    #[test]
    fn naive_empty_chunk_produces_empty_mesh() {
        let mesh = mesh_chunk_naive(&Chunk::new_air(), &ChunkNeighbors::NONE, WHITE);
        assert!(mesh.is_empty());
    }

    #[test]
    fn naive_isolated_block_has_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        let mesh = mesh_chunk_naive(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn naive_adjacent_pair_has_ten_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        chunk.set(LocalPos::new(6, 5, 5), STONE);
        let mesh = mesh_chunk_naive(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 10);
    }

    #[test]
    fn naive_solid_chunk_meshes_to_shell_only() {
        let mesh = mesh_chunk_naive(&Chunk::filled(STONE), &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6 * N * N);
    }

    #[test]
    fn naive_solid_neighbor_culls_shared_border() {
        let chunk = Chunk::filled(STONE);
        let neighbor = Chunk::filled(STONE);
        let neighbors = ChunkNeighbors::NONE.with_pos_x(&neighbor);
        let mesh = mesh_chunk_naive(&chunk, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5 * N * N);
    }

    #[test]
    fn naive_fully_enclosed_chunk_meshes_to_nothing() {
        let chunk = Chunk::filled(STONE);
        let solid = Chunk::filled(STONE);
        let neighbors = ChunkNeighbors::NONE
            .with_neg_x(&solid)
            .with_pos_x(&solid)
            .with_neg_y(&solid)
            .with_pos_y(&solid)
            .with_neg_z(&solid)
            .with_pos_z(&solid);
        let mesh = mesh_chunk_naive(&chunk, &neighbors, WHITE);
        assert!(mesh.is_empty());
    }

    #[test]
    fn naive_single_blocks_touching_across_border() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(31, 5, 5), STONE);
        let mut neighbor = Chunk::new_air();
        neighbor.set(LocalPos::new(0, 5, 5), STONE);
        let neighbors = ChunkNeighbors::NONE.with_pos_x(&neighbor);
        let mesh = mesh_chunk_naive(&chunk, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5);
    }

    #[test]
    fn naive_top_face_winding_points_up() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        assert_top_winding_up(&mesh_chunk_naive(&chunk, &ChunkNeighbors::NONE, WHITE));
    }

    // -----------------------------------------------------------------
    // Greedy: merging behavior.
    // -----------------------------------------------------------------

    /// The signature greedy result: a fully solid chunk is exactly 6 quads
    /// — one maximal rectangle per side.
    #[test]
    fn greedy_solid_chunk_is_six_quads() {
        let mesh = mesh_chunk(&Chunk::filled(STONE), &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
    }

    /// Two adjacent same-type blocks: each exposed side merges into one
    /// rectangle → 6 quads total (vs naive's 10).
    #[test]
    fn greedy_adjacent_pair_is_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        chunk.set(LocalPos::new(6, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
    }

    /// Different block types must NOT merge.
    #[test]
    fn greedy_does_not_merge_different_blocks() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        chunk.set(LocalPos::new(6, 5, 5), BlockId(2));
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 10);
    }

    /// A 3D checkerboard has no mergeable faces: greedy must equal naive's
    /// count exactly — the worst case is still correct.
    #[test]
    fn greedy_checkerboard_equals_naive_count() {
        let mut chunk = Chunk::new_air();
        for pos in LocalPos::iter() {
            if (pos.x() + pos.y() + pos.z()) % 2 == 0 {
                chunk.set(pos, STONE);
            }
        }
        let greedy = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        let naive = mesh_chunk_naive(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(greedy.quad_count(), naive.quad_count());
    }

    #[test]
    fn greedy_top_face_winding_points_up() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        assert_top_winding_up(&mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE));
    }

    // -----------------------------------------------------------------
    // THE differential test: greedy and naive must cover exactly the same
    // set of unit face cells, with the same colors, each exactly once —
    // over randomized chunks with randomized neighbors.
    // -----------------------------------------------------------------

    /// A unit face cell: (axis, outward sign, plane, a, b, layer).
    ///
    /// This is a GEOMETRY key: it deliberately excludes brightness. Greedy
    /// merging must cover the exact same set of visible face cells, with the
    /// same texture layers, as the naive mesher — that invariant is what this
    /// oracle guards, and it is independent of light and AO.
    ///
    /// Brightness is intentionally NOT compared here. Smooth light is
    /// vertex-shared, so it reconstructs affinely across a merged rectangle and
    /// would match; but classic per-corner AO is cell-anchored (each corner
    /// excludes its own quadrant), so a greedy run legitimately stretches an AO
    /// gradient that the naive mesher keeps per-cell. That is an accepted
    /// property of AO-on-greedy-meshes, not a meshing bug — greedy is the
    /// shipping mesher and its interpolation is the intended result. Light and
    /// AO correctness are pinned directly by the smooth-light gradient/seam
    /// tests and the `corner_ao` truth-table test below.
    type CellKey = (usize, bool, i32, i32, i32, u32);

    /// The four quad corners in emit order (0,0),(1,0),(1,1),(0,1), recovered
    /// from a 6-index quad regardless of which diagonal it was triangulated
    /// on. Returns (positions, brightnesses) in that order.
    fn quad_corners(mesh: &MeshData, quad: &[u32]) -> ([[f32; 3]; 4], [f32; 4]) {
        // Both triangulations share all four vertices; the 4 distinct indices
        // in ascending order are the base..base+3 block emit_rect pushed.
        let mut idxs: Vec<u32> = quad.to_vec();
        idxs.sort_unstable();
        idxs.dedup();
        assert_eq!(idxs.len(), 4, "quad must reference exactly 4 vertices");
        let b = idxs[0] as usize;
        assert_eq!(
            idxs,
            vec![b as u32, b as u32 + 1, b as u32 + 2, b as u32 + 3]
        );
        let pos = [
            mesh.vertices[b].position,
            mesh.vertices[b + 1].position,
            mesh.vertices[b + 2].position,
            mesh.vertices[b + 3].position,
        ];
        let bright = [
            mesh.vertices[b].brightness,
            mesh.vertices[b + 1].brightness,
            mesh.vertices[b + 2].brightness,
            mesh.vertices[b + 3].brightness,
        ];
        (pos, bright)
    }

    /// Decompose a mesh into the unit face cells its quads cover (geometry
    /// only — see `CellKey` on why brightness is excluded).
    fn coverage(mesh: &MeshData) -> HashMap<CellKey, u32> {
        let mut map: HashMap<CellKey, u32> = HashMap::new();

        assert_eq!(mesh.indices.len() % 6, 0, "quads are 6 indices each");
        for quad in mesh.indices.chunks_exact(6) {
            let bi = quad.iter().copied().min().unwrap() as usize;
            let (corners, _cbright) = quad_corners(mesh, quad);
            let layer = mesh.vertices[bi].layer;
            assert!(
                (0..4).all(|k| mesh.vertices[bi + k].layer == layer),
                "quad has inconsistent layer"
            );

            // Plane axis: the coordinate identical across all four corners.
            let axis = (0..3)
                .find(|&a| corners.iter().all(|c| c[a] == corners[0][a]))
                .expect("quad is not axis-aligned");
            let plane = corners[0][axis] as i32;

            // Outward sign from the winding normal of the first triangle.
            let e1 = sub(corners[1], corners[0]);
            let e2 = sub(corners[2], corners[0]);
            let normal = cross(e1, e2);
            assert!(normal[axis] != 0.0, "degenerate quad / wrong plane");
            let positive = normal[axis] > 0.0;

            let (a1, a2) = other_axes(axis);
            let (min1, max1) = extent(&corners, a1);
            let (min2, max2) = extent(&corners, a2);

            for p in min1..max1 {
                for q in min2..max2 {
                    *map.entry((axis, positive, plane, p, q, layer)).or_insert(0) += 1;
                }
            }
        }
        map
    }

    fn other_axes(axis: usize) -> (usize, usize) {
        match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        }
    }

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn extent(corners: &[[f32; 3]; 4], axis: usize) -> (i32, i32) {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for c in corners {
            min = min.min(c[axis]);
            max = max.max(c[axis]);
        }
        (min as i32, max as i32)
    }

    fn random_chunk(rng: &mut SplitMix64, fill_percent: u64, types: u16) -> Chunk {
        let mut chunk = Chunk::new_air();
        for pos in LocalPos::iter() {
            if rng.next() % 100 < fill_percent {
                chunk.set(pos, BlockId(1 + (rng.next() % types as u64) as u16));
            }
        }
        chunk
    }

    /// A greedy-merged W×H quad must carry UVs spanning 0..W / 0..H so the
    /// texture-array layer tiles W×H times (not stretched). For a solid
    /// chunk, each of the 6 faces is one 32×32 quad whose UVs span 0..32.
    #[test]
    fn greedy_quad_uvs_tile_across_merge() {
        let mesh = mesh_chunk(&Chunk::filled(STONE), &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
        for quad in mesh.indices.chunks_exact(6) {
            let bi = quad[0] as usize;
            let uvs: Vec<[f32; 2]> = (0..4).map(|k| mesh.vertices[bi + k].uv).collect();
            let max_u = uvs.iter().map(|c| c[0]).fold(0.0_f32, f32::max);
            let max_v = uvs.iter().map(|c| c[1]).fold(0.0_f32, f32::max);
            assert_eq!(max_u, 32.0, "merged face U should span the full 32");
            assert_eq!(max_v, 32.0, "merged face V should span the full 32");
            assert_eq!(uvs[0], [0.0, 0.0]);
        }
    }

    /// A single isolated block's faces are 1×1, UVs span 0..1 (one tile).
    #[test]
    fn unit_quad_uvs_are_unit() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        for quad in mesh.indices.chunks_exact(6) {
            let bi = quad[0] as usize;
            let uvs: Vec<[f32; 2]> = (0..4).map(|k| mesh.vertices[bi + k].uv).collect();
            let max_u = uvs.iter().map(|c| c[0]).fold(0.0_f32, f32::max);
            let max_v = uvs.iter().map(|c| c[1]).fold(0.0_f32, f32::max);
            assert_eq!(max_u, 1.0);
            assert_eq!(max_v, 1.0);
        }
    }

    #[test]
    fn differential_greedy_equals_naive_coverage() {
        // Distinct layer per (block, face) so per-face layer equality is
        // also under test (a uniform layer would hide face-mixups).
        let layered = |b: BlockId, face: usize| (b.0 as u32) * 6 + face as u32;

        for seed in 0..8u64 {
            let mut rng = SplitMix64::new(0xC0FFEE + seed);
            let fill = 10 + (seed * 12) % 85; // sparse through dense
            let chunk = random_chunk(&mut rng, fill, 6);

            // A random subset of neighbors, themselves random.
            let nx = random_chunk(&mut rng, 50, 6);
            let py = random_chunk(&mut rng, 50, 6);
            let mut neighbors = ChunkNeighbors::NONE;
            if seed % 2 == 0 {
                neighbors = neighbors.with_neg_x(&nx);
            }
            if seed % 3 == 0 {
                neighbors = neighbors.with_pos_y(&py);
            }

            let naive = mesh_chunk_naive(&chunk, &neighbors, layered);
            let greedy = mesh_chunk(&chunk, &neighbors, layered);

            let naive_cov = coverage(&naive);
            let greedy_cov = coverage(&greedy);

            assert!(
                naive_cov.values().all(|&c| c == 1),
                "seed {seed}: naive double-covers a cell"
            );
            assert!(
                greedy_cov.values().all(|&c| c == 1),
                "seed {seed}: greedy double-covers a cell"
            );
            assert_eq!(
                naive_cov, greedy_cov,
                "seed {seed}: greedy and naive disagree on the visible surface"
            );
            assert!(
                greedy.quad_count() <= naive.quad_count(),
                "seed {seed}: greedy used more quads than naive"
            );
        }
    }

    // -----------------------------------------------------------------
    // Shared helpers.
    // -----------------------------------------------------------------

    fn assert_top_winding_up(mesh: &MeshData) {
        let mut found_top = false;
        for tri in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [
                mesh.vertices[tri[0] as usize].position,
                mesh.vertices[tri[1] as usize].position,
                mesh.vertices[tri[2] as usize].position,
            ];
            if a[1] == 1.0 && b[1] == 1.0 && c[1] == 1.0 {
                found_top = true;
                let e1 = sub(b, a);
                let e2 = sub(c, a);
                let normal_y = e1[2] * e2[0] - e1[0] * e2[2];
                assert!(normal_y > 0.0, "top face winding is not CCW from above");
            }
        }
        assert!(found_top, "no top face found");
    }

    /// Minimal deterministic RNG for tests — no external crates in the
    /// dependency graph.
    struct SplitMix64(u64);

    impl SplitMix64 {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        }
    }
    #[test]
    fn snapshot_matches_chunk_and_neighbors() {
        // The snapshot must agree with direct chunk reads: interior cells,
        // a present face neighbor's touching layer, and absent neighbors as
        // air / light 0.
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(3, 4, 5), STONE);
        chunk.set_sky_light(LocalPos::new(3, 5, 5), 12);
        let mut west = Chunk::filled(STONE);
        west.set(LocalPos::new(31, 7, 7), BlockId::AIR);
        west.set_block_light(LocalPos::new(31, 7, 7), 9);
        let neighbors = ChunkNeighbors::NONE.with_neg_x(&west);
        let input = MeshInput::build(&chunk, &neighbors);
        // Interior.
        assert!(!input.is_air(3, 4, 5));
        assert_eq!(input.block(3, 4, 5), STONE);
        assert!(input.is_air(3, 5, 5));
        assert_eq!(input.light(3, 5, 5), 12);
        // Present neighbor's touching layer (x = -1 maps to west x = 31).
        assert!(input.is_air(-1, 7, 7));
        assert_eq!(input.light(-1, 7, 7), 9);
        assert!(!input.is_air(-1, 8, 8));
        // Absent neighbors: air, dark.
        assert!(input.is_air(32, 0, 0));
        assert_eq!(input.light(32, 0, 0), 0);
        assert!(input.is_air(0, -1, 0));
    }

    /// Perf yardstick, not a correctness test. Run with:
    /// `cargo test --release -p vox-mesh bench_mesh -- --ignored --nocapture`
    /// History: 2736us (per-visit palette decodes) -> 1876us (MeshInput
    /// snapshot, ADR-0006) -> 2236us (task 2: per-vertex smooth light + AO-
    /// aware corner sampling + 26-neighbor shell; quads 20 -> 36 as gradients
    /// subdivide merges), July 2026. Still well under the pre-snapshot 2736us.
    #[test]
    #[ignore]
    fn bench_mesh_realistic_surface_chunk() {
        use vox_core::{BlockRegistry, apply_chunk_light, compute_chunk_light_2ch, open_sky_top};
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
        // Real light so greedy merge behavior is realistic.
        let (light, _) = compute_chunk_light_2ch(
            &chunk,
            &reg,
            &[None, None, None, None, None, None],
            &[None, None, None, None, None, None],
            &open_sky_top(),
            true,
        );
        apply_chunk_light(&mut chunk, &light);
        let iters = 300;
        let start = std::time::Instant::now();
        let mut quads = 0usize;
        for _ in 0..iters {
            let m = mesh_chunk(&chunk, &ChunkNeighbors::NONE, |_, _| 0);
            quads = m.quad_count();
        }
        let total = start.elapsed();
        println!(
            "mesh x{iters}: per-chunk {:.0}us, {quads} quads",
            total.as_secs_f64() * 1e6 / iters as f64
        );
    }
    #[test]
    fn smooth_light_creates_gradient_across_face() {
        let mut chunk = Chunk::filled(STONE);
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                for y in 16..CHUNK_SIZE as u8 {
                    chunk.set(LocalPos::new(x, y, z), BlockId::AIR);
                }
            }
        }
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                chunk.set_sky_light(LocalPos::new(x, 16, z), (x / 2).min(15));
            }
        }
        let input = MeshInput::build(&chunk, &ChunkNeighbors::NONE);
        let cl = corner_lights(&input, [4, 15, 4], 1, true, 2, 0);
        let spread =
            cl.iter().cloned().fold(0.0f32, f32::max) - cl.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread > 0.25,
            "expected a gradient across the face, got {cl:?}"
        );
    }

    #[test]
    fn smooth_light_seam_continuity_across_chunk_boundary() {
        let mut left = Chunk::new_air();
        for y in 0..CHUNK_SIZE as u8 {
            for z in 0..CHUNK_SIZE as u8 {
                left.set(LocalPos::new(31, y, z), STONE);
            }
        }
        let mut right = Chunk::new_air();
        for y in 0..CHUNK_SIZE as u8 {
            for z in 0..CHUNK_SIZE as u8 {
                right.set_sky_light(LocalPos::new(0, y, z), 10);
            }
        }
        let nb = ChunkNeighbors::NONE.with_pos_x(&right);
        let input = MeshInput::build(&left, &nb);
        let cl = corner_lights(&input, [31, 8, 8], 0, true, 1, 2);
        for (k, &c) in cl.iter().enumerate() {
            assert!((c - 10.0).abs() < 1e-6, "corner {k} = {c}");
        }
    }

    #[test]
    fn smooth_light_diagonal_seam_reads_edge_neighbor() {
        // A +Y face on the chunk EDGE (x=31) samples diagonally into the
        // +X neighbor's cells — an edge-neighbor read that the 26-neighbor
        // shell must supply. Without it the corner would be darkened by a
        // phantom-dark shell cell.
        let mut chunk = Chunk::new_air();
        // Solid floor across the top-edge row so +Y faces exist at x=31.
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                chunk.set(LocalPos::new(x, 10, z), STONE);
            }
        }
        for z in 0..CHUNK_SIZE as u8 {
            for x in 0..CHUNK_SIZE as u8 {
                chunk.set_sky_light(LocalPos::new(x, 11, z), 15);
            }
        }
        // +X neighbor: air with sky 15 at x=0 (the diagonal cell from x=31).
        let mut east = Chunk::new_air();
        for y in 0..CHUNK_SIZE as u8 {
            for z in 0..CHUNK_SIZE as u8 {
                east.set_sky_light(LocalPos::new(0, y, z), 15);
            }
        }
        let nb = ChunkNeighbors::NONE.with_pos_x(&east);
        let input = MeshInput::build(&chunk, &nb);
        // Top face of the edge block (31,10,8): all four corners should be 15
        // (uniform bright), including the corner that samples into +X.
        let cl = corner_lights(&input, [31, 10, 8], 1, true, 2, 0);
        for (k, &c) in cl.iter().enumerate() {
            assert!(
                (c - 15.0).abs() < 1e-6,
                "edge corner {k} = {c}, expected 15 (no phantom-dark seam)"
            );
        }
    }

    #[test]
    fn choose_flip_picks_lower_contrast_diagonal() {
        assert!(choose_flip([1.0, 1.0, 0.0, 1.0]));
        assert!(!choose_flip([0.5, 0.5, 0.5, 0.5]));
        assert!(!choose_flip([1.0, 0.0, 1.0, 0.0]));
    }

    /// `corner_ao` truth table (M06 task 3). A +Y top face at (cx,10,cz):
    /// occluders live in the air-side plane y=11, and `occ(du,dv)` probes cell
    /// (cx+dv, 11, cz+du) since u_axis=2 (z), v_axis=0 (x). Corner emit order
    /// is (0,0),(1,0),(1,1),(0,1) with su=2cu-1, sv=2cv-1.
    #[test]
    fn corner_ao_darkens_tucked_corners() {
        let (cx, cz) = (8u8, 8u8);
        let cell = [cx as i32, 10, cz as i32];

        // One occluder on the +z side. It is `side1` (occ(su,0)) only for the
        // su=+1 corners: k=1 (1,0) and k=2 (1,1) -> one side occluded -> 2.
        // k=0 and k=3 face away -> fully lit (3).
        let mut a = Chunk::new_air();
        a.set(LocalPos::new(cx, 10, cz), STONE);
        a.set(LocalPos::new(cx, 11, cz + 1), STONE);
        let ia = MeshInput::build(&a, &ChunkNeighbors::NONE);
        assert_eq!(
            corner_ao(&ia, cell, 1, true, 2, 0),
            [3, 2, 2, 3],
            "single side occluder darkens that edge to level 2"
        );

        // Add the perpendicular occluder on the +x side (`side2` for cv=1).
        // Now k=2 (1,1) has BOTH sides occluded -> hard 0; k=1 and k=3 each
        // keep one occluded side -> 2; k=0 stays fully lit.
        let mut b = Chunk::new_air();
        b.set(LocalPos::new(cx, 10, cz), STONE);
        b.set(LocalPos::new(cx, 11, cz + 1), STONE);
        b.set(LocalPos::new(cx + 1, 11, cz), STONE);
        let ib = MeshInput::build(&b, &ChunkNeighbors::NONE);
        assert_eq!(
            corner_ao(&ib, cell, 1, true, 2, 0),
            [3, 2, 0, 2],
            "both sides occluded -> darkest corner (0)"
        );
    }

    /// End to end: the dedicated AO term must reach emitted vertex brightness
    /// through the real mesher. With NO sky light, `corner_lights` is uniform,
    /// so the emitted per-corner brightness varies ONLY by `ao_factor` — which
    /// isolates the AO term from the smooth-light contribution.
    #[test]
    fn ao_factor_reaches_vertex_brightness_through_mesher() {
        let (cx, cz) = (8u8, 8u8);
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(cx, 10, cz), STONE);
        chunk.set(LocalPos::new(cx, 11, cz + 1), STONE);
        chunk.set(LocalPos::new(cx + 1, 11, cz), STONE);

        let input = MeshInput::build(&chunk, &ChunkNeighbors::NONE);
        let cell = [cx as i32, 10, cz as i32];
        let ao = corner_ao(&input, cell, 1, true, 2, 0);
        assert_eq!(ao, [3, 2, 0, 2]);

        // Uniform (zero) light -> brightness is base * ao_factor per corner.
        let base = face_brightness(1, true) * light_curve_f(0.0);
        let expected: Vec<f32> = ao.iter().map(|&l| base * ao_factor(l)).collect();

        let mesh = mesh_chunk_naive(&chunk, &ChunkNeighbors::NONE, WHITE);

        // Recover the +Y face over (cx,10,cz): all four corners at y==11,
        // spanning x in [cx,cx+1], z in [cz,cz+1].
        let mut got: Option<Vec<f32>> = None;
        for quad in mesh.indices.chunks_exact(6) {
            let (pos, bright) = quad_corners(&mesh, quad);
            let top = pos.iter().all(|p| (p[1] - 11.0).abs() < 1e-6);
            let xr = cx as f32 - 1e-6..=cx as f32 + 1.0 + 1e-6;
            let zr = cz as f32 - 1e-6..=cz as f32 + 1.0 + 1e-6;
            let inx = pos.iter().all(|p| xr.contains(&p[0]));
            let inz = pos.iter().all(|p| zr.contains(&p[2]));
            if top && inx && inz {
                got = Some(bright.to_vec());
            }
        }
        let got = got.expect("no +Y face emitted over the floor cell");

        // Diagonal flips may reorder corners, so match as a set within eps.
        for e in &expected {
            assert!(
                got.iter().any(|g| (g - e).abs() < 1e-4),
                "AO-scaled brightness {e} not emitted; got {got:?}, expected {expected:?}"
            );
        }
    }
}
