mod store;

#[cfg(test)]
pub use self::store::exists;
pub use self::store::{init, read, write};

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;
/// Size of block id in bytes.
pub const BLOCK_ID_SIZE: usize = 32;

define_array_wrapper! {
    /// Unique id of a block.
    pub struct BlockId([u8; BLOCK_ID_SIZE]);
}

derive_rand_for_wrapper!(BlockId);
derive_sqlx_traits_for_u8_array_wrapper!(BlockId);
