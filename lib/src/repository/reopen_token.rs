use crate::{
    access_control::AccessSecrets,
    crypto::sign,
    error::{Error, Result},
};
use bincode::Options;
use serde::{Deserialize, Serialize};

/// Token which can be obtained from an open repository and which can then be used to reopen the
/// repository without having to provide the local secret.
#[derive(Serialize, Deserialize)]
pub struct ReopenToken {
    pub(super) secrets: AccessSecrets,
    pub(super) writer_id: sign::PublicKey,
}

impl ReopenToken {
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
