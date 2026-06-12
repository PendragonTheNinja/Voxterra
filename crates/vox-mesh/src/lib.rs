//! vox-mesh: chunk → triangle mesh conversion.
//!
//! Headless crate: depends only on vox-core (+ bytemuck for byte casting).
//! Must never depend on wgpu/winit — meshing is pure data transformation
//! and is unit-tested without a GPU.
//!
//! Milestone 00: naive *culled* meshing — one quad per solid-block face
//! that borders a non-solid block. Greedy meshing (merging coplanar quads)
//! replaces this in Milestone 01; the [`MeshData`] output type is the
//! stable contract between the two.

use vox_core::{BlockId, CHUNK_SIZE, Chunk, LocalPos};

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

/// Mesh a single chunk with culled meshing.
///
/// `color_of` maps a (non-air) block to its base RGB color; block→color
/// policy deliberately lives with the caller, not in this crate.
///
/// MILESTONE 00 LIMITATION: neighbors outside this chunk are treated as
/// air, so faces on chunk borders are always emitted. Correct cross-chunk
/// culling needs neighbor data and arrives with the multi-chunk world
/// (Milestone 01+).
pub fn mesh_chunk(chunk: &Chunk, mut color_of: impl FnMut(BlockId) -> [f32; 3]) -> MeshData {
    let mut mesh = MeshData::default();
    let size = CHUNK_SIZE as i32;

    for pos in LocalPos::iter() {
        let block = chunk.get(pos);
        if block.is_air() {
            continue;
        }
        let base_color = color_of(block);
        let (x, y, z) = (pos.x() as i32, pos.y() as i32, pos.z() as i32);

        for face in &FACES {
            let (nx, ny, nz) = (x + face.offset[0], y + face.offset[1], z + face.offset[2]);

            let neighbor_is_air =
                if (0..size).contains(&nx) && (0..size).contains(&ny) && (0..size).contains(&nz) {
                    chunk
                        .get(LocalPos::new(nx as u8, ny as u8, nz as u8))
                        .is_air()
                } else {
                    true // outside the chunk: treat as air (see doc comment)
                };

            if neighbor_is_air {
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

    #[test]
    fn empty_chunk_produces_empty_mesh() {
        let mesh = mesh_chunk(&Chunk::new_air(), WHITE);
        assert!(mesh.is_empty());
        assert!(mesh.vertices.is_empty());
    }

    /// Acceptance criterion: one isolated block → exactly 6 quads.
    #[test]
    fn isolated_block_has_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, WHITE);
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    /// Acceptance criterion: two adjacent blocks → 10 quads (the two
    /// touching faces are culled).
    #[test]
    fn adjacent_pair_has_ten_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(5, 5, 5), STONE);
        chunk.set(LocalPos::new(6, 5, 5), STONE);
        let mesh = mesh_chunk(&chunk, WHITE);
        assert_eq!(mesh.quad_count(), 10);
    }

    /// A block on the chunk corner: out-of-chunk neighbors count as air,
    /// so all 6 faces are emitted.
    #[test]
    fn corner_block_has_six_quads() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        let mesh = mesh_chunk(&chunk, WHITE);
        assert_eq!(mesh.quad_count(), 6);
    }

    /// A completely solid chunk meshes to exactly its outer shell:
    /// 6 sides × 32×32 faces. Interior faces must all be culled.
    #[test]
    fn solid_chunk_meshes_to_shell_only() {
        let mesh = mesh_chunk(&Chunk::filled(STONE), WHITE);
        assert_eq!(mesh.quad_count(), 6 * 32 * 32);
    }

    /// Every index must reference a real vertex, and indices come in
    /// whole triangles.
    #[test]
    fn indices_are_valid() {
        let mut chunk = Chunk::new_air();
        for x in 0..8 {
            for z in 0..8 {
                chunk.set(LocalPos::new(x, 3, z), STONE);
            }
        }
        let mesh = mesh_chunk(&chunk, WHITE);
        assert_eq!(mesh.indices.len() % 3, 0);
        let max = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&i| i < max));
    }

    /// Winding check: for every triangle, the cross product of its edges
    /// must have positive length (no degenerate triangles), and for the
    /// known top face of a single block it must point up (+Y).
    #[test]
    fn top_face_winding_points_up() {
        let mut chunk = Chunk::new_air();
        chunk.set(LocalPos::new(0, 0, 0), STONE);
        let mesh = mesh_chunk(&chunk, WHITE);

        // Find a triangle whose three vertices all sit at y == 1.0 — that's
        // the top face. Its winding normal must be +Y.
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
}
