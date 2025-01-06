use core::str;
use std::{fmt, io};

use hmac::{
    digest::{typenum::Unsigned, OutputSizeUser},
    Hmac, Mac,
};
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
};

use crate::transport::ClientError;

// # Authentication protocol
//
// `psk`              : pre-shared key
// `challenge`        : 256 bytes long randomly generated byte string
// `client_challenge` : challenge generated by the client
// `server_challenge` : challenge generated by the server
// `server_proof`     : HMAC-SHA-256(psk, client_challenge)
// `client_proof`     : HMAC_SHA-256(psk, server_challenge)
//
// 1. Client sends `client_challenge` to the server
// 2. Server sends `server_proof` followed by `server_challenge` to the client
// 3. Client verifies `server_proof`. If it's valid, it send `client_proof` to the server. Otherwise
// it closes the connection and returns "authentication failed" error.
// 4. Server verifies `client_proof`. If it's valid, the client is authenticated. Otherwise it
// closes the connection.
//
const AUTH_CHALLENGE_SIZE: usize = 256;
const AUTH_PROOF_SIZE: usize = <<Sha256 as OutputSizeUser>::OutputSize as Unsigned>::USIZE; // 32

#[derive(Clone, Copy)]
pub struct AuthKey([u8; Self::SIZE]);

impl AuthKey {
    pub const SIZE: usize = 32;

    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        Self(rng.gen())
    }

    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AuthKey").finish_non_exhaustive()
    }
}

impl Serialize for AuthKey {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if s.is_human_readable() {
            let mut buffer = [0; Self::SIZE * 2];
            // unwrap is OK because the buffer has sufficient length.
            hex::encode_to_slice(self.0, &mut buffer).unwrap();

            // unwrap is OK because the buffer contains only hex digits which are valid utf-8.
            str::from_utf8(&buffer).unwrap().serialize(s)
        } else {
            serde_bytes::serialize(&self.0, s)
        }
    }
}

impl<'de> Deserialize<'de> for AuthKey {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if d.is_human_readable() {
            let hex = <&str>::deserialize(d)?;
            let mut bytes = [0; Self::SIZE];
            hex::decode_to_slice(hex, &mut bytes).map_err(|_| {
                D::Error::invalid_length(hex.len(), &format!("{}", Self::SIZE * 2).as_str())
            })?;

            Ok(Self(bytes))
        } else {
            serde_bytes::deserialize(d).map(Self)
        }
    }
}

pub(super) async fn server(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    auth_key: &AuthKey,
) -> io::Result<()> {
    let mut client_challenge = [0; AUTH_CHALLENGE_SIZE];
    reader.read_exact(&mut client_challenge).await?;

    let server_proof = hmac(auth_key, &client_challenge).finalize();

    let mut server_challenge = [0; AUTH_CHALLENGE_SIZE];
    OsRng.fill(&mut server_challenge);

    writer.write_all(&server_proof.into_bytes()).await?;
    writer.write_all(&server_challenge).await?;

    let mut client_proof = [0; AUTH_PROOF_SIZE];
    reader.read_exact(&mut client_proof).await?;

    hmac(auth_key, &server_challenge)
        .verify_slice(&client_proof)
        .map_err(|_| io::Error::other("client authentication failed"))?;

    Ok(())
}

pub(super) async fn client(
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    auth_key: &AuthKey,
) -> Result<(), ClientError> {
    let mut client_challenge = [0; AUTH_CHALLENGE_SIZE];
    OsRng.fill(&mut client_challenge);
    writer.write_all(&client_challenge).await?;

    let mut server_proof = [0; AUTH_PROOF_SIZE];
    reader.read_exact(&mut server_proof).await?;

    hmac(auth_key, &client_challenge)
        .verify_slice(&server_proof)
        .map_err(|_| ClientError::Authentication)?;

    let mut server_challenge = [0; AUTH_CHALLENGE_SIZE];
    reader.read_exact(&mut server_challenge).await?;

    let client_proof = hmac(auth_key, &server_challenge).finalize();
    writer.write_all(&client_proof.into_bytes()).await?;

    Ok(())
}

fn hmac(key: &AuthKey, message: &[u8]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .expect("invalid HMAC key size")
        .chain_update(message)
}
