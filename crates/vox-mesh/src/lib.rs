//! vox-mesh: chunk → triangle mesh conversion.
//!
//! Headless crate: depends only on vox-core (+ bytemuck for byte casting).
//! Must never depend on wgpu/winit — meshing is pure data transformation
//! and is unit-tested without a GPU.
//!
//! Milestone 01 state: naive *culled* meshing with cross-chunk culling —
//! a chunk is meshed together with views of its six face-neighbors, so no
//! quads are emitted between solid blocks across a chunk border. Greedy
//! meshing (merging coplanar quads) is task 3; [`MeshData`] is the stable
//! contract between the two.

use vox_core::{BlockId, CHUNK_SIZE, Chunk, ChunkPos, LocalPos, World};

/// One mesh vertex. `repr(C)` + Pod so vox-render can cast the vertex
/// buffer straight to bytes for GPU upload.
///
/// Milestone 00 format: position + final color (block color × face
/// brightness, baked CPU-side). This will be redesigned when textures
/// arrive — expected, fine.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
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

/// The six cube faces. Order is arbitrary but stable.
///
/// Corner order per face is counter-clockwise when viewed from outside the
/// cube (right-handed coords, Y up), so wgpu's default `FrontFace::Ccw`
/// culling works. Verified by hand: for each face, (c1−c0)×(c2−c0) equals
/// the outward normal. Don't reorder casually.
struct Face {
    /// Neighbor offset this face points toward.
    offset: [i32; 3],
    /// Corner positions relative to the block's min corner.
    corners: [[f32; 3]; 4],
    /// Directional shading factor (top brightest, bottom darkest) so
    /// geometry reads as 3D before real lighting exists.
    brightness: f32,
}

const FACES: [Face; 6] = [
    // +X (east)
    Face {
        offset: [1, 0, 0],
        corners: [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
        brightness: 0.75,
    },
    // -X (west)
    Face {
        offset: [-1, 0, 0],
        corners: [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
        ],
        brightness: 0.65,
    },
    // +Y (top)
    Face {
        offset: [0, 1, 0],
        corners: [
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
        brightness: 1.0,
    },
    // -Y (bottom)
    Face {
        offset: [0, -1, 0],
        corners: [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
        brightness: 0.45,
    },
    // +Z (south)
    Face {
        offset: [0, 0, 1],
        corners: [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        brightness: 0.8,
    },
    // -Z (north)
    Face {
        offset: [0, 0, -1],
        corners: [
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
        ],
        brightness: 0.6,
    },
];

/// Is the block at chunk-relative coords (x, y, z) air? Coordinates one
/// step outside `0..32` resolve into the corresponding neighbor (only one
/// axis can be out of range, since face offsets are unit steps). Missing
/// neighbors count as air.
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

/// Mesh a single chunk with culled meshing, consulting `neighbors` for
/// faces on chunk borders.
///
/// `color_of` maps a (non-air) block to its base RGB color; block→color
/// policy deliberately lives with the caller, not in this crate.
pub fn mesh_chunk(
    chunk: &Chunk,
    neighbors: &ChunkNeighbors,
    mut color_of: impl FnMut(BlockId) -> [f32; 3],
) -> MeshData {
    let mut mesh = MeshData::default();

    // Uniform-air fast path: most chunks in a cubic-chunk world.
    if chunk.is_all_air() {
        return mesh;
    }

    for pos in LocalPos::iter() {
        let block = chunk.get(pos);
        if block.is_air() {
            continue;
        }
        let base_color = color_of(block);
        let (x, y, z) = (pos.x() as i32, pos.y() as i32, pos.z() as i32);

        for face in &FACES {
            let neighbor_air = is_air_at(
                chunk,
                neighbors,
                x + face.offset[0],
                y + face.offset[1],
                z + face.offset[2],
            );
            if neighbor_air {
                emit_quad(&mut mesh, [x as f32, y as f32, z as f32], face, base_color);
            }
        }
    }

    mesh
}

fn emit_quad(mesh: &mut MeshData, block_min: [f32; 3], face: &Face, base_color: [f32; 3]) {
    let base_index = mesh.vertices.len() as u32;
    let color = [
        base_color[0] * face.brightness,
        base_color[1] * face.brightness,
        base_color[2] * face.brightness,
    ];

    for corner in &face.corners {
        mesh.vertices.push(Vertex {
            position: [
                block_min[0] + corner[0],
                block_min[1] + corner[1],
                block_min[2] + corner[2],
            ],
            color,
        });
    }

    // Two CCW triangles: (0,1,2) and (0,2,3).
    mesh.indices.extend_from_slice(&[
        base_index,
        base_index + 1,
        base_index + 2,
        base_index,
        base_index + 2,
        base_index + 3,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: fn(BlockId) -> [f32; 3] = |_| [1.0, 1.0, 1.0];
    const STONE: BlockId = BlockId(1);
    const N: usize = CHUNK_SIZE;

    // --- Milestone 00 behavior, preserved with NONE neighbors. ---

    #[test]
    fn empty_chunk_produces_empty_mesh() {
        let mesh = mesh_chunk(&Chunk::new_air(), &ChunkNeighbors::NONE, WHITE);
        assert!(mesh.is_empty());
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn isolated_block_has_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn adjacent_pair_has_ten_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        chunk.set(LocalPos::new(6, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 10);
    }

    #[test]
    fn corner_block_without_neighbors_has_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6);
    }

    #[test]
    fn solid_chunk_meshes_to_shell_only() {
        let mesh = mesh_chunk(&Chunk::filled(STONE), &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.quad_count(), 6 * N * N);
    }

    #[test]
    fn indices_are_valid() {
        let mut chunk = Chunk::new_air();
        for x in 0..8 {
            for z in 0..8 {
                chunk.set(LocalPos::new(x, 3, z), STONE);
            }
        }
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(mesh.indices.len() % 3, 0);
        let max = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&i| i < max));
    }

    #[test]
    fn top_face_winding_points_up() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        let mesh = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);

        let mut found_top = false;
        for tri in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [
                mesh.vertices[tri[0] as usize].position,
                mesh.vertices[tri[1] as usize].position,
                mesh.vertices[tri[2] as usize].position,
            ];
            if a[1] == 1.0 && b[1] == 1.0 && c[1] == 1.0 {
                found_top = true;
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let normal_y = e1[2] * e2[0] - e1[0] * e2[2];
                assert!(normal_y > 0.0, "top face winding is not CCW from above");
            }
        }
        assert!(found_top, "no top face found");
    }

    // --- Milestone 01: cross-chunk culling. ---

    /// A solid chunk with one solid neighbor: the shared border emits no
    /// faces. 6 sides of 32×32, minus the one shared side.
    #[test]
    fn solid_neighbor_culls_shared_border() {
        let chunk = Chunk::filled(STONE);
        let neighbor = Chunk::filled(STONE);
        let neighbors = ChunkNeighbors {
            pos_x: Some(&neighbor),
            ..ChunkNeighbors::NONE
        };
        let mesh = mesh_chunk(&chunk, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5 * N * N);
    }

    /// Fully enclosed by solid neighbors: nothing to draw at all.
    #[test]
    fn fully_enclosed_chunk_meshes_to_nothing() {
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
        let mesh = mesh_chunk(&chunk, &neighbors, WHITE);
        assert!(mesh.is_empty());
    }

    /// An all-air neighbor that exists must behave exactly like a missing
    /// neighbor: the border faces are emitted.
    #[test]
    fn air_neighbor_equals_missing_neighbor() {
        let chunk = Chunk::filled(STONE);
        let air = Chunk::new_air();
        let neighbors = ChunkNeighbors {
            pos_x: Some(&air),
            ..ChunkNeighbors::NONE
        };
        let with_air = mesh_chunk(&chunk, &neighbors, WHITE);
        let with_none = mesh_chunk(&chunk, &ChunkNeighbors::NONE, WHITE);
        assert_eq!(with_air.quad_count(), with_none.quad_count());
    }

    /// Two single blocks touching across a chunk border: each loses
    /// exactly the touching face.
    #[test]
    fn single_blocks_touching_across_border() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(31, 5, 5), STONE);
        let mut neighbor = Chunk::new_air();
        neighbor.set(LocalPos::new(0, 5, 5), STONE);

        let neighbors = ChunkNeighbors {
            pos_x: Some(&neighbor),
            ..ChunkNeighbors::NONE
        };
        let mesh = mesh_chunk(&chunk, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5);
    }

    /// ChunkNeighbors::of gathers the right chunks from a World,
    /// including across negative coordinates.
    #[test]
    fn neighbors_of_world() {
        use vox_core::WorldPos;
        let mut world = vox_core::World::new();
        // Fill chunk (0,0,0) and its +X neighbor (1,0,0) solid.
        world.insert_chunk(ChunkPos::new(0, 0, 0), Chunk::filled(STONE));
        world.insert_chunk(ChunkPos::new(1, 0, 0), Chunk::filled(STONE));
        // Sanity: the world agrees blocks exist on both sides of the border.
        assert!(!world.get_block(WorldPos::new(31, 0, 0)).is_air());
        assert!(!world.get_block(WorldPos::new(32, 0, 0)).is_air());

        let center = world.chunk(ChunkPos::new(0, 0, 0)).unwrap();
        let neighbors = ChunkNeighbors::of(&world, ChunkPos::new(0, 0, 0));
        assert!(neighbors.pos_x.is_some());
        assert!(neighbors.neg_x.is_none());

        let mesh = mesh_chunk(center, &neighbors, WHITE);
        assert_eq!(mesh.quad_count(), 5 * N * N);
    }
}
