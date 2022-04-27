use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub(super) const VERSION: Version = Version(0);

define_byte_array_wrapper! {
    /// Randomly generated ephemeral id that uniquely identifies a replica. Changes every time the
    /// replica is restarted.
    pub(super) struct RuntimeId([u8; 16]);
}
derive_rand_for_wrapper!(RuntimeId);

impl RuntimeId {
    pub async fn read_from<R>(io: &mut R) -> io::Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let mut id = [0; Self::SIZE];
        io.read_exact(&mut id).await?;

        Ok(Self(id))
    }

    pub async fn write_into<W>(&self, io: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        io.write_all(&self.0).await
    }
}

/// Protocol version
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub(super) struct Version(u8);

impl Version {
    pub async fn read_from<R>(io: &mut R) -> io::Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        Ok(Self(io.read_u8().await?))
    }

    pub async fn write_into<W>(&self, io: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        io.write_u8(self.0).await
    }
}

impl std::convert::From<Version> for u32 {
    fn from(v: Version) -> u32 {
        v.0 as u32
    }
}
