//! Block registry (Milestone 03 task 1).
//!
//! The single source of truth for what each [`BlockId`] *is*: its name,
//! its physical and visual properties, and which texture-array layer each of
//! its six faces uses. Replaces the placeholder hardcoded block constants that were
//! scattered across vox-worldgen and vox-app.
//!
//! Headless: no graphics dependency. The renderer consumes only the
//! resolved texture-array layer indices the registry provides; vox-mesh and
//! vox-worldgen depend on this module through vox-core.
//!
//! ## Stable IDs
//!
//! `STONE = 1`, `DIRT = 2`, `GRASS = 3` are unchanged from the placeholder
//! era ON PURPOSE: saved worlds (Milestone 02) store these numeric ids, so
//! preserving them keeps existing persisted edits valid. New blocks take
//! higher ids. Air is always 0.
//!
//! ## Face order
//!
//! `faces[i]` is indexed the same way vox-mesh's `FACE_DIRS` orders faces:
//! `0:+X (east) 1:-X (west) 2:+Y (top) 3:-Y (bottom) 4:+Z (south) 5:-Z (north)`.
//! Keep these in lockstep — the mesher uses this index to pick the layer.
//!
//! ## Three properties, not one (ADR-0011)
//!
//! `solid` once meant five different things at its call sites. It is now
//! three independent properties, and every caller asks the one it means:
//!
//! | property | question | asked by |
//! |---|---|---|
//! | `solid` | does it stop the player / can it be targeted? | collision, raycast, break/place, the terrain top the LOD draws |
//! | `opaque` | does it block light / hide the face behind it? | lighting, the skylight heightmap, meshing, AO |
//! | `renders` | does it have geometry at all? | meshing |
//!
//! Every block is either air (none of the three), an opaque cube (all three),
//! or water — rendered, but neither solid nor opaque. A new call site that picks the wrong property compiles, passes most
//! tests, and shows up in play — choose by the question being asked.

use crate::block::BlockId;

/// Face index constants, matching vox-mesh's `FACE_DIRS` order.
pub mod face {
    pub const POS_X: usize = 0;
    pub const NEG_X: usize = 1;
    pub const POS_Y: usize = 2; // top
    pub const NEG_Y: usize = 3; // bottom
    pub const POS_Z: usize = 4;
    pub const NEG_Z: usize = 5;
}

/// Definition of one block type.
#[derive(Debug, Clone)]
pub struct BlockType {
    /// Human-readable name (debug/UI/logging).
    pub name: &'static str,
    /// PHYSICAL: stops the player, can be targeted and broken, and counts as
    /// the terrain top. See the module docs for which property to ask.
    pub solid: bool,
    /// VISUAL: blocks light and hides the neighbouring face behind it.
    pub opaque: bool,
    /// VISUAL: has geometry. Only air does not.
    pub renders: bool,
    /// Texture-array layer index for each of the six faces (see module docs
    /// for face order). Used by meshing/rendering from task 2 onward.
    pub faces: [u32; 6],
    /// INTERIM (task 1): flat debug color, used until the texture pipeline
    /// (task 2) replaces flat colors with atlas/array sampling. Remove when
    /// the mesher emits texture coords instead of colors.
    pub color: [f32; 3],
    /// Block-light this block emits (0..=15), 0 for non-emitters. Seeds the
    /// lighting flood-fill (Milestone 04, ADR-0004).
    pub light_emission: u8,
}

impl BlockType {
    /// A block with the same texture layer on all six faces.
    const fn uniform(name: &'static str, layer: u32, color: [f32; 3]) -> Self {
        Self {
            name,
            solid: true,
            opaque: true,
            renders: true,
            faces: [layer; 6],
            color,
            light_emission: 0,
        }
    }

    /// A block with distinct top / side / bottom layers (e.g. grass).
    const fn tsb(name: &'static str, top: u32, side: u32, bottom: u32, color: [f32; 3]) -> Self {
        // Order: +X, -X, +Y(top), -Y(bottom), +Z, -Z.
        Self {
            name,
            solid: true,
            opaque: true,
            renders: true,
            faces: [side, side, top, bottom, side, side],
            color,
            light_emission: 0,
        }
    }

    /// A uniform-textured block that emits block light (e.g. a lamp).
    const fn emitter(name: &'static str, layer: u32, color: [f32; 3], emission: u8) -> Self {
        Self {
            name,
            solid: true,
            opaque: true,
            renders: true,
            faces: [layer; 6],
            color,
            light_emission: emission,
        }
    }

    /// Water (ADR-0011): drawn in the transparent pass, passes light
    /// undimmed (M10), and stops nobody — the player falls to the seabed.
    fn water() -> Self {
        Self {
            name: "water",
            solid: false,
            opaque: false,
            renders: true,
            faces: [L_WATER; 6],
            color: [0.20, 0.38, 0.62],
            light_emission: 0,
        }
    }

    fn air() -> Self {
        Self {
            name: "air",
            solid: false,
            opaque: false,
            renders: false,
            faces: [0; 6],
            color: [0.0, 0.0, 0.0],
            light_emission: 0,
        }
    }
}

// --- Stable block ids. Air is 0; STONE/DIRT/GRASS keep their M01/M02 ids. ---
pub const AIR: BlockId = BlockId::AIR;
pub const STONE: BlockId = BlockId(1);
pub const DIRT: BlockId = BlockId(2);
pub const GRASS: BlockId = BlockId(3);
pub const SAND: BlockId = BlockId(4);
pub const COBBLESTONE: BlockId = BlockId(5);
pub const PLANKS: BlockId = BlockId(6);
/// A light-emitting block (Milestone 04). Solid, glows at near-max level.
pub const LAMP: BlockId = BlockId(7);
/// Sea water (M10 A1, ADR-0011). Rendered, not solid, not opaque.
pub const WATER: BlockId = BlockId(8);

// --- Texture-array layer assignment (one layer per distinct tile). Used by
// the texture pipeline in task 2; declared here so the registry is the
// single source of truth. ---
const L_STONE: u32 = 0;
const L_DIRT: u32 = 1;
const L_GRASS_TOP: u32 = 2;
const L_GRASS_SIDE: u32 = 3;
const L_SAND: u32 = 4;
const L_COBBLE: u32 = 5;
const L_PLANKS: u32 = 6;
const L_LAMP: u32 = 7;
/// Water: translucent (the tile carries alpha), near and far alike — the LOD
/// draws the same surface over its own seabed (ADR-0011 decision 1c).
pub const L_WATER: u32 = 8;
/// Number of texture-array layers the default registry references. The
/// renderer builds exactly this many, so a new layer is one change here and
/// one tile in `vox-render`'s `tile_pixels`.
pub const DEFAULT_LAYER_COUNT: u32 = 9;

/// Block-light level emitted by the lamp block.
pub const LAMP_EMISSION: u8 = 14;

/// The registry of all known block types, indexed by `BlockId`.
pub struct BlockRegistry {
    types: Vec<BlockType>,
}

impl BlockRegistry {
    /// Build the default starter registry. Indices are dense and match the
    /// `BlockId` numeric values, so lookup is O(1) indexing.
    pub fn default_set() -> Self {
        // Order MUST match the BlockId values (0..=8).
        let types = vec![
            BlockType::air(),                                         // 0 AIR
            BlockType::uniform("stone", L_STONE, [0.55, 0.55, 0.58]), // 1
            BlockType::uniform("dirt", L_DIRT, [0.55, 0.40, 0.27]),   // 2
            BlockType::tsb(
                "grass",
                L_GRASS_TOP,
                L_GRASS_SIDE,
                L_DIRT,
                [0.33, 0.62, 0.28],
            ), // 3
            BlockType::uniform("sand", L_SAND, [0.82, 0.76, 0.55]),   // 4
            BlockType::uniform("cobblestone", L_COBBLE, [0.50, 0.50, 0.52]), // 5
            BlockType::uniform("planks", L_PLANKS, [0.62, 0.46, 0.28]), // 6
            BlockType::emitter("lamp", L_LAMP, [1.0, 0.93, 0.70], LAMP_EMISSION), // 7
            BlockType::water(),                                       // 8
        ];
        Self { types }
    }

    /// Look up a block type. Unknown ids resolve to air (defensive: a saved
    /// world referencing a since-removed id won't crash, it reads as empty).
    #[inline]
    pub fn get(&self, id: BlockId) -> &BlockType {
        self.types.get(id.0 as usize).unwrap_or(&self.types[0])
    }

    /// PHYSICAL: stops the player, can be targeted and broken, counts as the
    /// terrain top. Not "blocks light" — that is [`is_opaque`](Self::is_opaque).
    #[inline]
    pub fn is_solid(&self, id: BlockId) -> bool {
        self.get(id).solid
    }

    /// VISUAL: blocks light and hides the neighbouring face behind it.
    #[inline]
    pub fn is_opaque(&self, id: BlockId) -> bool {
        self.get(id).opaque
    }

    /// VISUAL: has geometry at all.
    #[inline]
    pub fn renders(&self, id: BlockId) -> bool {
        self.get(id).renders
    }

    /// Block-light this block emits (0..=15), 0 for non-emitters.
    #[inline]
    pub fn emission(&self, id: BlockId) -> u8 {
        self.get(id).light_emission
    }

    /// Texture-array layer for a given face (see module docs for face index).
    #[inline]
    pub fn face_layer(&self, id: BlockId, face_index: usize) -> u32 {
        self.get(id).faces[face_index]
    }

    /// INTERIM (task 1): flat color for a block, until textures land.
    #[inline]
    pub fn color(&self, id: BlockId) -> [f32; 3] {
        self.get(id).color
    }

    /// All placeable block ids (everything solid), for block-selection UI in
    /// task 4. Excludes air.
    pub fn placeable(&self) -> impl Iterator<Item = BlockId> + '_ {
        (0..self.types.len())
            .map(|i| BlockId(i as u16))
            .filter(move |&id| self.get(id).solid)
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

impl Default for BlockRegistry {
    fn default() -> Self {
        Self::default_set()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable() {
        // These must never change — saved worlds depend on them (M02).
        assert_eq!(STONE, BlockId(1));
        assert_eq!(DIRT, BlockId(2));
        assert_eq!(GRASS, BlockId(3));
        assert_eq!(AIR, BlockId(0));
    }

    #[test]
    fn registry_indices_match_ids() {
        let reg = BlockRegistry::default_set();
        assert_eq!(reg.get(STONE).name, "stone");
        assert_eq!(reg.get(DIRT).name, "dirt");
        assert_eq!(reg.get(GRASS).name, "grass");
        assert_eq!(reg.get(SAND).name, "sand");
        assert_eq!(reg.get(COBBLESTONE).name, "cobblestone");
        assert_eq!(reg.get(PLANKS).name, "planks");
    }

    #[test]
    fn air_is_not_solid_others_are() {
        let reg = BlockRegistry::default_set();
        assert!(!reg.is_solid(AIR));
        assert!(reg.is_solid(STONE));
        assert!(reg.is_solid(GRASS));
    }

    /// The three properties are what they claim for every block, and the
    /// shortcut the mesher relies on holds: air is the only block that does
    /// not render, so `is_air` answers "renders nothing".
    #[test]
    fn properties_are_consistent() {
        let reg = BlockRegistry::default_set();
        assert!(!reg.is_opaque(AIR) && !reg.renders(AIR));
        for i in 0..reg.len() {
            let id = BlockId(i as u16);
            assert_eq!(reg.renders(id), !id.is_air(), "{}", reg.get(id).name);
            // Opaque implies renders: nothing hides a face while drawing none.
            assert!(
                !reg.is_opaque(id) || reg.renders(id),
                "{}",
                reg.get(id).name
            );
        }
        for id in [STONE, DIRT, GRASS, SAND, COBBLESTONE, PLANKS, LAMP] {
            assert!(reg.is_solid(id) && reg.is_opaque(id) && reg.renders(id));
        }
    }

    /// Water is the block the split exists for: drawn, but it stops nobody
    /// and blocks no light — and it is not something to place from the
    /// hotbar.
    #[test]
    fn water_renders_but_is_neither_solid_nor_opaque() {
        let reg = BlockRegistry::default_set();
        assert_eq!(reg.get(WATER).name, "water");
        assert!(reg.renders(WATER));
        assert!(!reg.is_solid(WATER), "the player would stand on the sea");
        assert!(!reg.is_opaque(WATER), "the seabed would be dark and hidden");
        assert!(!reg.placeable().any(|b| b == WATER));
    }

    #[test]
    fn grass_has_distinct_faces() {
        let reg = BlockRegistry::default_set();
        let top = reg.face_layer(GRASS, face::POS_Y);
        let bottom = reg.face_layer(GRASS, face::NEG_Y);
        let side = reg.face_layer(GRASS, face::POS_X);
        assert_eq!(top, L_GRASS_TOP);
        assert_eq!(side, L_GRASS_SIDE);
        assert_eq!(bottom, L_DIRT); // grass sits on dirt
        assert_ne!(top, side);
        assert_ne!(side, bottom);
        // Side faces are all the same.
        assert_eq!(reg.face_layer(GRASS, face::NEG_X), side);
        assert_eq!(reg.face_layer(GRASS, face::POS_Z), side);
        assert_eq!(reg.face_layer(GRASS, face::NEG_Z), side);
    }

    #[test]
    fn uniform_block_same_on_all_faces() {
        let reg = BlockRegistry::default_set();
        for f in 0..6 {
            assert_eq!(reg.face_layer(STONE, f), L_STONE);
        }
    }

    #[test]
    fn unknown_id_resolves_to_air() {
        let reg = BlockRegistry::default_set();
        let unknown = BlockId(9999);
        assert_eq!(reg.get(unknown).name, "air");
        assert!(!reg.is_solid(unknown));
    }

    #[test]
    fn all_face_layers_within_array_bounds() {
        let reg = BlockRegistry::default_set();
        for i in 0..reg.len() {
            for f in 0..6 {
                let layer = reg.face_layer(BlockId(i as u16), f);
                assert!(layer < DEFAULT_LAYER_COUNT, "layer {layer} out of bounds");
            }
        }
    }

    #[test]
    fn placeable_excludes_air() {
        let reg = BlockRegistry::default_set();
        let placeable: Vec<BlockId> = reg.placeable().collect();
        assert!(!placeable.contains(&AIR));
        assert!(placeable.contains(&STONE));
        assert_eq!(placeable.len(), 7); // stone..lamp
    }

    #[test]
    fn lamp_emits_others_dark() {
        let reg = BlockRegistry::default_set();
        assert_eq!(reg.emission(LAMP), LAMP_EMISSION);
        assert!(reg.emission(LAMP) > 0);
        assert_eq!(reg.emission(STONE), 0);
        assert_eq!(reg.emission(GRASS), 0);
        assert_eq!(reg.emission(AIR), 0);
    }

    #[test]
    fn lamp_is_placeable_and_solid() {
        let reg = BlockRegistry::default_set();
        assert!(reg.is_solid(LAMP));
        let placeable: Vec<BlockId> = reg.placeable().collect();
        assert!(placeable.contains(&LAMP));
    }
}

#[cfg(test)]
mod color_parity {
    use super::*;
    #[test]
    fn interim_colors_match_milestone02_values() {
        let reg = BlockRegistry::default_set();
        assert_eq!(reg.color(STONE), [0.55, 0.55, 0.58]);
        assert_eq!(reg.color(DIRT), [0.55, 0.40, 0.27]);
        assert_eq!(reg.color(GRASS), [0.33, 0.62, 0.28]);
    }
}
