use crate::transport::TransportError;
use ouisync_lib::{crypto::sign::Signature, RepositoryId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Mirror repository on a remote server
    Mirror {
        repository_id: RepositoryId,
        /// `SessionCookie` signed by the repo write key. Used as a zero-knowledge proof that the
        /// client has write access to the repository.
        proof: Signature,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    None,
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum ServerError {
    #[error("server is shutting down")]
    ShuttingDown,
    #[error("invalid argument")]
    InvalidArgument,
    #[error("transport error")]
    Transport(#[from] TransportError),
    #[error("failed to create repository: {0}")]
    CreateRepository(String),
}
