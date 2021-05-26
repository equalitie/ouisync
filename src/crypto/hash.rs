use crate::format;
use sha3::{
    digest::{
        generic_array::{sequence::GenericSequence, typenum::Unsigned, GenericArray},
        Digest,
    },
    Sha3_256,
};
use std::{
    array::TryFromSliceError,
    convert::{TryFrom, TryInto},
    fmt,
};

/// Wrapper for a 256-bit hash digest, for convenience. Also implements friendly formatting.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[repr(transparent)]
pub struct Hash(Inner);

impl Hash {
    pub const SIZE: usize = <Inner as GenericSequence<_>>::Length::USIZE;

    pub fn null() -> Self {
        Self(Inner::default())
    }

    pub fn is_null(&self) -> bool {
        self.0.iter().all(|&byte| byte == 0)
    }
}

impl From<Inner> for Hash {
    fn from(inner: Inner) -> Self {
        Hash(inner)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

impl TryFrom<&'_ [u8]> for Hash {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let slice: [u8; Self::SIZE] = slice.try_into()?;
        Ok(Self(slice.into()))
    }
}

derive_sqlx_type_for_u8_array_wrapper!(Hash);
derive_sqlx_encode_for_u8_array_wrapper!(Hash);
derive_sqlx_decode_for_u8_array_wrapper!(Hash);

type Inner = GenericArray<u8, <Sha3_256 as Digest>::OutputSize>;

/// Trait for types that can be cryptographically hashed.
pub trait Hashable {
    fn hash(&self) -> Hash;
}

impl Hashable for &'_ [u8] {
    fn hash(&self) -> Hash {
        Sha3_256::digest(self).into()
    }
}

impl Hashable for u64 {
    fn hash(&self) -> Hash {
        (&self.to_le_bytes()[..]).hash()
    }
}
