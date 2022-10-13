use crate::crypto::sign::{Keypair, PublicKey, SecretKey, Signature};
use rand::{rngs::OsRng, Rng};
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// These structures are used to generate ephemeral id that uniquely identifies a replica. Changes
/// every time the replica is restarted. The cryptography involved is to ensure one replica can't
/// claim to be another one.

pub struct SecretRuntimeId {
    secret: SecretKey,
    public: PublicKey,
}

impl SecretRuntimeId {
    pub fn generate() -> Self {
        let Keypair { secret, public } = Keypair::random();

        Self { secret, public }
    }

    pub fn public(&self) -> PublicRuntimeId {
        PublicRuntimeId {
            public: self.public,
        }
    }

    pub fn as_public_key(&self) -> &PublicKey {
        &self.public
    }
}

#[derive(PartialEq, Eq, Ord, PartialOrd, Hash, Clone, Copy, Deserialize, Serialize, Debug)]
pub struct PublicRuntimeId {
    public: PublicKey,
}

impl PublicRuntimeId {
    async fn read_from<R>(io: &mut R) -> io::Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let bytes = read_bytes::<{ PublicKey::SIZE }, R>(io).await?;
        Ok(Self {
            public: PublicKey::from(bytes),
        })
    }

    async fn write_into<W>(&self, io: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        io.write_all(self.public.as_ref()).await
    }

    pub fn as_public_key(&self) -> &PublicKey {
        &self.public
    }
}

impl AsRef<[u8]> for PublicRuntimeId {
    fn as_ref(&self) -> &[u8] {
        self.public.as_ref()
    }
}

pub async fn exchange<IO>(
    our_runtime_id: &SecretRuntimeId,
    io: &mut IO,
) -> io::Result<PublicRuntimeId>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    let our_challenge: [u8; 32] = OsRng.gen();

    io.write_all(&our_challenge).await?;
    our_runtime_id.public().write_into(io).await?;

    let their_challenge = read_bytes::<32, IO>(io).await?;
    let their_runtime_id = PublicRuntimeId::read_from(io).await?;

    let our_signature = our_runtime_id
        .secret
        .sign(&to_sign(&their_challenge), &our_runtime_id.public);

    io.write_all(our_signature.as_ref()).await?;

    let their_signature = read_bytes::<{ Signature::SIZE }, IO>(io).await?;
    let their_signature = Signature::from(their_signature);

    if !their_runtime_id
        .public
        .verify(&to_sign(&our_challenge), &their_signature)
    {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Failed to verify runtime ID",
        ));
    }

    Ok(their_runtime_id)
}

const TO_SIGN_PREFIX: &[u8; 10] = b"runtime-id";

fn to_sign(buf: &[u8; 32]) -> [u8; 32 + TO_SIGN_PREFIX.len()] {
    let mut out = [0u8; 32 + TO_SIGN_PREFIX.len()];
    out[..TO_SIGN_PREFIX.len()].clone_from_slice(TO_SIGN_PREFIX);
    out[TO_SIGN_PREFIX.len()..].clone_from_slice(buf);
    out
}

async fn read_bytes<const N: usize, R>(io: &mut R) -> io::Result<[u8; N]>
where
    R: AsyncRead + Unpin,
{
    let mut out = [0u8; N];
    io.read_exact(&mut out).await?;
    Ok(out)
}
