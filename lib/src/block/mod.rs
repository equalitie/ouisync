use crate::crypto::{Digest, Hashable};

mod store;

#[cfg(test)]
pub use self::store::exists;
pub use self::store::{init, read, write};

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;

define_byte_array_wrapper! {
    /// Unique id of a block.
    pub struct BlockId([u8; 32]);
}

derive_rand_for_wrapper!(BlockId);
derive_sqlx_traits_for_byte_array_wrapper!(BlockId);

impl Hashable for BlockId {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state)
    }
}
