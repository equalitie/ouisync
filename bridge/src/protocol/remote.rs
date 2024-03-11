use crate::transport::TransportError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod v0 {
    use super::*;
    use ouisync_lib::ShareToken;

    #[derive(Debug, Serialize, Deserialize)]
    pub enum Request {
        Mirror { share_token: ShareToken },
    }
}

pub mod v1 {
    use super::*;
    use ouisync_lib::{crypto::sign::Signature, RepositoryId};

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
}

#[derive(Debug, Serialize, Deserialize)]
// NOTE: using untagged to support old clients that don't support versioning.
#[serde(untagged)]
pub enum Request {
    V0(v0::Request),
    V1(v1::Request),
}

impl From<v0::Request> for Request {
    fn from(v0: v0::Request) -> Self {
        Self::V0(v0)
    }
}

impl From<v1::Request> for Request {
    fn from(v1: v1::Request) -> Self {
        Self::V1(v1)
    }
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
