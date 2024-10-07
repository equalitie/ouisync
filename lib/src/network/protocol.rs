use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// First string in a handshake, helps with weeding out connections with completely different
// protocols on the other end.
pub(super) const MAGIC: &[u8; 7] = b"OUISYNC";
pub(super) const VERSION: Version = Version(13);

/// Protocol version
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub(super) struct Version(u64);

impl Version {
    pub async fn read_from<R>(io: &mut R) -> io::Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let mut buffer = [0; vint64::MAX_BYTES];
        io.read_exact(&mut buffer[..1]).await?;

        let len = vint64::decoded_len(buffer[0]);
        io.read_exact(&mut buffer[1..len]).await?;

        let value = vint64::decode(&mut &buffer[..])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;

        Ok(Self(value))
    }

    pub async fn write_into<W>(&self, io: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        io.write_all(vint64::encode(self.0).as_ref()).await
    }
}

impl std::convert::From<Version> for u32 {
    fn from(v: Version) -> u32 {
        v.0 as u32
    }
}
