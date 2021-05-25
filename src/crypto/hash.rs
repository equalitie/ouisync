use crate::format;
use sha3::{
    digest::{
        generic_array::{sequence::GenericSequence, typenum::Unsigned, GenericArray},
        Digest,
    },
    Sha3_256,
};
use sqlx::{
    error::BoxDynError,
    sqlite::{Sqlite, SqliteTypeInfo, SqliteValueRef},
    Decode, Type,
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
        for c in self.0.as_slice() {
            if *c != 0 {
                return false;
            }
        }
        true
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

impl<'r> Decode<'r, Sqlite> for Hash {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let slice = <&[u8]>::decode(value)?;
        Ok(slice.try_into()?)
    }
}

impl Type<Sqlite> for Hash {
    fn type_info() -> SqliteTypeInfo {
        <&[u8] as Type<Sqlite>>::type_info()
    }
}

type Inner = GenericArray<u8, <Sha3_256 as Digest>::OutputSize>;
