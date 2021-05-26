mod store;

#[cfg(test)]
pub use self::store::exists;
pub use self::store::{init, read, write};

use crate::format;
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use std::{
    array::TryFromSliceError,
    convert::{TryFrom, TryInto},
    fmt,
};

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;
/// Size of block id in bytes.
pub const BLOCK_ID_SIZE: usize = 32;

/// Unique id of a block.
#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockId([u8; BLOCK_ID_SIZE]);

impl BlockId {
    /// Generate a random id using the default RNG ([`rand::thread_rng`]).
    pub fn random() -> Self {
        rand::thread_rng().gen()
    }
}

impl TryFrom<&'_ [u8]> for BlockId {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(slice.try_into()?))
    }
}

impl AsRef<[u8]> for BlockId {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Distribution<BlockId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlockId {
        BlockId(self.sample(rng))
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

derive_sqlx_traits_for_u8_array_wrapper!(BlockId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_id_fmt() {
        let id = BlockId([
            0x00, 0x01, 0x02, 0x03, 0x05, 0x07, 0x0b, 0x0d, 0x11, 0x13, 0x17, 0x1d, 0x1f, 0x25,
            0x29, 0x2b, 0x2f, 0x35, 0x3b, 0x3d, 0x43, 0x47, 0x49, 0x4f, 0x53, 0x59, 0x61, 0x65,
            0x67, 0x6b, 0x6d, 0x71,
        ]);

        assert_eq!(
            format!("{:x}", id),
            "0001020305070b0d1113171d1f25292b2f353b3d4347494f53596165676b6d71"
        );
        assert_eq!(format!("{:1x}", id), "");
        assert_eq!(format!("{:2x}", id), "..");
        assert_eq!(format!("{:3x}", id), "..");
        assert_eq!(format!("{:4x}", id), "00..");
        assert_eq!(format!("{:6x}", id), "0001..");
        assert_eq!(format!("{:8x}", id), "000102..");

        assert_eq!(format!("{:?}", id), "000102..");
        assert_eq!(
            format!("{}", id),
            "0001020305070b0d1113171d1f25292b2f353b3d4347494f53596165676b6d71"
        );
    }
}
