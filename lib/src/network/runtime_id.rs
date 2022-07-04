use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::crypto::sign::{Keypair, PublicKey, SecretKey};
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{Deserialize, Serialize};

/// These structures are used to generate ephemeral id that uniquely identifies a replica. Changes
/// every time the replica is restarted. The cryptography involved is to ensure one replica can't
/// claim to be another one.

pub struct SecretRuntimeId {
    secret: SecretKey,
    public: PublicKey
}

impl SecretRuntimeId {
    pub fn generate() -> Self {
        let Keypair { secret, public } = Keypair::random();

        Self {
            secret,
            public
        }
    }

    pub fn public(&self) -> PublicRuntimeId {
        PublicRuntimeId {
            public: self.public,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Copy, Deserialize, Serialize, Debug)]
pub struct PublicRuntimeId {
    public: PublicKey,
}

impl PublicRuntimeId {
    pub async fn read_from<R>(io: &mut R) -> io::Result<Self>
    where
        R: AsyncRead + Unpin,
    {
        let mut bytes = [0; PublicKey::SIZE];
        io.read_exact(&mut bytes).await?;
        Ok(Self{ public: PublicKey::from(bytes) })
    }

    pub async fn write_into<W>(&self, io: &mut W) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        io.write_all(self.public.as_ref()).await
    }
}

impl AsRef<[u8]> for PublicRuntimeId {
    fn as_ref(&self) -> &[u8] {
        self.public.as_ref()
    }
}
