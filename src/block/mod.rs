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
/// Size of block name in bytes.
pub const BLOCK_NAME_SIZE: usize = 16;
/// Size of block version in bytes.
pub const BLOCK_VERSION_SIZE: usize = 16;

/// Unique name of a block which doesn't change when the block is modified.
#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockName([u8; BLOCK_NAME_SIZE]);

impl BlockName {
    /// Generate a random name using the default RNG ([`rand::thread_rng`]).
    pub fn random() -> Self {
        rand::thread_rng().gen()
    }
}

impl TryFrom<&'_ [u8]> for BlockName {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(slice.try_into()?))
    }
}

impl AsRef<[u8]> for BlockName {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Distribution<BlockName> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlockName {
        BlockName(self.sample(rng))
    }
}

impl fmt::Display for BlockName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for BlockName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for BlockName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

/// Block version which changes every time the block is modified.
///
/// Note: does not carry any information about temporal or causal succession, i.e. it's not possible
/// to tell which of a given two versions is "newer", only whether they are different.
#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BlockVersion([u8; BLOCK_VERSION_SIZE]);

impl BlockVersion {
    /// Generate a random version using the default RNG ([`rand::thread_rng`]).
    pub fn random() -> Self {
        rand::thread_rng().gen()
    }
}

impl TryFrom<&'_ [u8]> for BlockVersion {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(slice.try_into()?))
    }
}

impl AsRef<[u8]> for BlockVersion {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Distribution<BlockVersion> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlockVersion {
        BlockVersion(self.sample(rng))
    }
}

impl fmt::Display for BlockVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for BlockVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for BlockVersion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct BlockId {
    pub name: BlockName,
    pub version: BlockVersion,
}

impl BlockId {
    pub fn random() -> Self {
        Self {
            name: BlockName::random(),
            version: BlockVersion::random(),
        }
    }

    pub fn to_array(&self) -> [u8; BLOCK_NAME_SIZE + BLOCK_VERSION_SIZE] {
        let mut array = [0; BLOCK_NAME_SIZE + BLOCK_VERSION_SIZE];
        array[..BLOCK_NAME_SIZE].copy_from_slice(self.name.as_ref());
        array[BLOCK_NAME_SIZE..].copy_from_slice(self.version.as_ref());
        array
    }
}

impl TryFrom<&[u8]> for BlockId {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let split_at = BLOCK_NAME_SIZE.min(slice.len());

        let name = BlockName::try_from(&slice[..split_at])?;
        let version = BlockVersion::try_from(&slice[split_at..])?;

        Ok(Self { name, version })
    }
}

derive_sqlx_type_for_u8_array_wrapper!(BlockId);
derive_sqlx_decode_for_u8_array_wrapper!(BlockId);

// NOTE: Can't derive `Encode` because it needs a way to borrow the undelying u8 array which
// `BlockId` can't do because the array is not actually contained in it but is created on demand
// instead.

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}:{:x}", self.name, self.version)
    }
}

impl fmt::Debug for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}:{:8x}", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_name_fmt() {
        let name = BlockName([
            0x00, 0x01, 0x02, 0x03, 0x05, 0x07, 0x0b, 0x0d, 0x11, 0x13, 0x17, 0x1d, 0x1f, 0x25,
            0x29, 0x2b,
        ]);

        assert_eq!(format!("{:x}", name), "0001020305070b0d1113171d1f25292b");
        assert_eq!(format!("{:1x}", name), "");
        assert_eq!(format!("{:2x}", name), "..");
        assert_eq!(format!("{:3x}", name), "..");
        assert_eq!(format!("{:4x}", name), "00..");
        assert_eq!(format!("{:6x}", name), "0001..");
        assert_eq!(format!("{:8x}", name), "000102..");

        assert_eq!(format!("{:?}", name), "000102..");
        assert_eq!(format!("{}", name), "0001020305070b0d1113171d1f25292b");
    }
}
