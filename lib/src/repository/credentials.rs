use super::metadata;
use crate::{
    access_control::AccessSecrets,
    crypto::sign,
    error::{Error, Result},
};
use bincode::Options;
use serde::{Deserialize, Serialize};

/// Credentials for accessing a repository.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub(super) secrets: AccessSecrets,
    pub(super) writer_id: sign::PublicKey,
}

impl Credentials {
    pub fn with_random_writer_id(secrets: AccessSecrets) -> Self {
        Self {
            secrets,
            writer_id: metadata::generate_writer_id(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        // unwrap is ok because serialization into a vector can't fail unless we have a bug in the
        // code.
        bincode::options().serialize(self).unwrap()
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        bincode::options()
            .deserialize(input)
            .map_err(|_| Error::MalformedData)
    }
}
