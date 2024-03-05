use crate::transport::TransportError;
use ouisync_lib::{crypto::sign::Signature, RepositoryId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Create a blind replica of the repository on the remote server
    Create {
        repository_id: RepositoryId,
        /// Zero-knowledge proof that the client has write access to the repository.
        /// Computed by signing `SessionCookie` with the repo write key.
        proof: Signature,
    },
    /// Delete the repository from the remote server
    Delete {
        repository_id: RepositoryId,
        /// Zero-knowledge proof that the client has write access to the repository.
        /// Computed by signing `SessionCookie` with the repo write key.
        proof: Signature,
    },
    /// Check that the repository exists on the remote server.
    Exists { repository_id: RepositoryId },
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum ServerError {
    #[error("server is shutting down")]
    ShuttingDown,
    #[error("transport error")]
    Transport(#[from] TransportError),
    #[error("permission denied")]
    PermissionDenied,
    #[error("not found")]
    NotFound,
    #[error("internal server error: {0}")]
    Internal(String),
}
