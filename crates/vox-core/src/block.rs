//! Block identity.

/// Identifies a block type. `BlockId(0)` is always air — this is a permanent
/// invariant relied on by storage, meshing, and serialization.
///
/// `u16` allows 65,536 block types. Block *state* (orientation, growth
/// stage, etc.) is a separate future concern and must not be packed into
/// this ID ad hoc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BlockId(pub u16);

impl BlockId {
    /// The empty block. Always ID 0.
    pub const AIR: BlockId = BlockId(0);

    #[inline]
    pub const fn is_air(self) -> bool {
        self.0 == 0
    }
}
