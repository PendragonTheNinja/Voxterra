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
#[derive(Default, Clone, Copy)]
pub struct ChunkNeighbors<'a> {
    pub neg_x: Option<&'a Chunk>,
    pub pos_x: Option<&'a Chunk>,
    pub neg_y: Option<&'a Chunk>,
    pub pos_y: Option<&'a Chunk>,
    pub neg_z: Option<&'a Chunk>,
    pub pos_z: Option<&'a Chunk>,
}

impl<'a> ChunkNeighbors<'a> {
    /// No neighbors: standalone-chunk behavior (all border faces emitted).
    pub const NONE: ChunkNeighbors<'static> = ChunkNeighbors {
        neg_x: None,
        pos_x: None,
        neg_y: None,
        pos_y: None,
        neg_z: None,
        pos_z: None,
    };

    /// Gather the six neighbors of `pos` from a world.
    pub fn of(world: &'a World, pos: ChunkPos) -> Self {
        let n = |dx: i64, dy: i64, dz: i64| {
            world.chunk(ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
        };
        Self {
            neg_x: n(-1, 0, 0),
            pos_x: n(1, 0, 0),
            neg_y: n(0, -1, 0),
            pos_y: n(0, 1, 0),
            neg_z: n(0, 0, -1),
            pos_z: n(0, 0, 1),
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

/// Is the block at chunk-relative coords air? Coordinates one step outside
/// `0..32` resolve into the corresponding neighbor (only one axis can be
/// out of range, since face offsets are unit steps). Missing neighbors
/// count as air.
fn is_air_at(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> bool {
    let size = CHUNK_SIZE as i32;
    let last = (size - 1) as u8;

    let (target, local) = if x < 0 {
        (neighbors.neg_x, LocalPos::new(last, y as u8, z as u8))
    } else if x >= size {
        (neighbors.pos_x, LocalPos::new(0, y as u8, z as u8))
    } else if y < 0 {
        (neighbors.neg_y, LocalPos::new(x as u8, last, z as u8))
    } else if y >= size {
        (neighbors.pos_y, LocalPos::new(x as u8, 0, z as u8))
    } else if z < 0 {
        (neighbors.neg_z, LocalPos::new(x as u8, y as u8, last))
    } else if z >= size {
        (neighbors.pos_z, LocalPos::new(x as u8, y as u8, 0))
    } else {
        return chunk.get(LocalPos::new(x as u8, y as u8, z as u8)).is_air();
    };

    match target {
        None => true,
        Some(neighbor) => neighbor.get(local).is_air(),
    }
}

/// Block-light level at chunk-relative coords, reaching into the appropriate
/// neighbor when one step outside `0..32` (mirrors [`is_air_at`]). Missing
/// neighbors read as dark (0). A face samples the light of the (air) cell it
/// looks into, so surfaces are lit by the light in front of them.
fn light_at(chunk: &Chunk, neighbors: &ChunkNeighbors, x: i32, y: i32, z: i32) -> u8 {
    let size = CHUNK_SIZE as i32;
    let last = (size - 1) as u8;

    let (target, local) = if x < 0 {
        (neighbors.neg_x, LocalPos::new(last, y as u8, z as u8))
    } else if x >= size {
        (neighbors.pos_x, LocalPos::new(0, y as u8, z as u8))
    } else if y < 0 {
        (neighbors.neg_y, LocalPos::new(x as u8, last, z as u8))
    } else if y >= size {
        (neighbors.pos_y, LocalPos::new(x as u8, 0, z as u8))
    } else if z < 0 {
        (neighbors.neg_z, LocalPos::new(x as u8, y as u8, last))
    } else if z >= size {
        (neighbors.pos_z, LocalPos::new(x as u8, y as u8, 0))
    } else {
        let p = LocalPos::new(x as u8, y as u8, z as u8);
        return chunk.block_light(p).max(chunk.sky_light(p));
    };

    match target {
        None => 0,
        Some(neighbor) => neighbor.block_light(local).max(neighbor.sky_light(local)),
    }
}

/// Map a 0..=15 light level to a brightness multiplier. Non-linear so the
/// falloff looks natural, with a small ambient floor so unlit areas are dim
/// but not pure black (caves stay barely navigable). Combined multiplicatively
/// with the directional face shading.
fn light_curve(level: u8) -> f32 {
    const AMBIENT: f32 = 0.06;
    let t = (level as f32) / 15.0;
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
    brightness: f32,
) {
    // UVs run 0..w / 0..h so the texture-array layer tiles w×h times across
    // a greedy-merged quad under a Repeat sampler (ADR-0003).
    let corner = |du: f32, dv: f32| Vertex {
        position: [
            base[0] + u_dir[0] * du + v_dir[0] * dv,
            base[1] + u_dir[1] * du + v_dir[1] * dv,
            base[2] + u_dir[2] * du + v_dir[2] * dv,
        ],
        uv: [du, dv],
        layer,
        brightness,
    };

    let base_index = mesh.vertices.len() as u32;
    mesh.vertices.push(corner(0.0, 0.0));
    mesh.vertices.push(corner(w, 0.0));
    mesh.vertices.push(corner(w, h));
    mesh.vertices.push(corner(0.0, h));
    mesh.indices.extend_from_slice(&[
        base_index,
        base_index + 1,
        base_index + 2,
        base_index,
        base_index + 2,
        base_index + 3,
    ]);
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
    if chunk.is_all_air() {
        return mesh;
    }

    for pos in LocalPos::iter() {
        let block = chunk.get(pos);
        if block.is_air() {
            continue;
        }
        let coords = [pos.x() as i32, pos.y() as i32, pos.z() as i32];

        for (face_index, &(axis, positive, u_axis, v_axis)) in FACE_DIRS.iter().enumerate() {
            let mut neighbor = coords;
            neighbor[axis] += if positive { 1 } else { -1 };
            if !is_air_at(chunk, neighbors, neighbor[0], neighbor[1], neighbor[2]) {
                continue;
            }

            // Positive faces sit at the far side of the block cell.
            let mut base = [coords[0] as f32, coords[1] as f32, coords[2] as f32];
            if positive {
                base[axis] += 1.0;
            }
            // Light the face by the cell it looks into (the air neighbor).
            let light = light_at(chunk, neighbors, neighbor[0], neighbor[1], neighbor[2]);
            let brightness = face_brightness(axis, positive) * light_curve(light);
            emit_rect(
                &mut mesh,
                base,
                axis_unit(u_axis),
                axis_unit(v_axis),
                1.0,
                1.0,
                layer_of(block, face_index),
                brightness,
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
    if chunk.is_all_air() {
        return mesh;
    }

    let size = CHUNK_SIZE;
    // Per-slice mask: each visible face cell carries its block AND the light
    // level of the air cell it faces. Faces only merge when BOTH match, so a
    // light gradient correctly subdivides the merged rectangles.
    let mut mask: Vec<Option<(BlockId, u8)>> = vec![None; size * size];
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

                    let block = chunk.get(LocalPos::new(
                        coords[0] as u8,
                        coords[1] as u8,
                        coords[2] as u8,
                    ));

                    let cell = &mut mask[at(u, v)];
                    if block.is_air() {
                        *cell = None;
                        continue;
                    }

                    let mut n = coords;
                    n[axis] += if positive { 1 } else { -1 };
                    if is_air_at(chunk, neighbors, n[0], n[1], n[2]) {
                        let light = light_at(chunk, neighbors, n[0], n[1], n[2]);
                        *cell = Some((block, light));
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
                    let (block, light) = key;

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

                    emit_rect(
                        &mut mesh,
                        base,
                        u_dir,
                        v_dir,
                        w as f32,
                        h as f32,
                        layer_of(block, face_index),
                        dir_shade * light_curve(light),
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
        let neighbors = ChunkNeighbors {
            pos_x: Some(&neighbor),
            ..ChunkNeighbors::NONE
        };
        let mesh = mesh_chunk_naive(&chunk, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5 * N * N);
    }

    #[test]
    fn naive_fully_enclosed_chunk_meshes_to_nothing() {
        let chunk = Chunk::filled(STONE);
        let solid = Chunk::filled(STONE);
        let neighbors = ChunkNeighbors {
            neg_x: Some(&solid),
            pos_x: Some(&solid),
            neg_y: Some(&solid),
            pos_y: Some(&solid),
            neg_z: Some(&solid),
            pos_z: Some(&solid),
        };
        let mesh = mesh_chunk_naive(&chunk, &neighbors, WHITE);
        assert!(mesh.is_empty());
    }

    #[test]
    fn naive_single_blocks_touching_across_border() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(31, 5, 5), STONE);
        let mut neighbor = Chunk::new_air();
        neighbor.set(LocalPos::new(0, 5, 5), STONE);
        let neighbors = ChunkNeighbors {
            pos_x: Some(&neighbor),
            ..ChunkNeighbors::NONE
        };
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
    type CellKey = (usize, bool, i32, i32, i32, u32, u32);

    /// Decompose a mesh into the unit face cells its quads cover.
    fn coverage(mesh: &MeshData) -> HashMap<CellKey, u32> {
        let mut map: HashMap<CellKey, u32> = HashMap::new();

        assert_eq!(mesh.indices.len() % 6, 0, "quads are 6 indices each");
        for quad in mesh.indices.chunks_exact(6) {
            // emit_rect's index pattern: b, b+1, b+2, b, b+2, b+3.
            let b = quad[0];
            assert_eq!(
                quad,
                [b, b + 1, b + 2, b, b + 2, b + 3],
                "unexpected quad index pattern"
            );
            let bi = b as usize;
            let corners: [[f32; 3]; 4] = [
                mesh.vertices[bi].position,
                mesh.vertices[bi + 1].position,
                mesh.vertices[bi + 2].position,
                mesh.vertices[bi + 3].position,
            ];
            let layer = mesh.vertices[bi].layer;
            let bright = mesh.vertices[bi].brightness.to_bits();
            // All four corners of a quad must share the layer.
            assert!(
                (0..4).all(|k| mesh.vertices[bi + k].layer == layer),
                "quad has inconsistent layer"
            );

            // Plane axis: the coordinate identical across all four corners.
            let axis = (0..3)
                .find(|&a| corners.iter().all(|c| c[a] == corners[0][a]))
                .expect("quad is not axis-aligned");
            let plane = corners[0][axis] as i32;

            // Outward sign from the winding normal.
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
                    *map.entry((axis, positive, plane, p, q, layer, bright))
                        .or_insert(0) += 1;
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
            let neighbors = ChunkNeighbors {
                neg_x: (seed % 2 == 0).then_some(&nx),
                pos_y: (seed % 3 == 0).then_some(&py),
                ..ChunkNeighbors::NONE
            };

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
}
