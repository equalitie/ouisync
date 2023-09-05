use crate::transport::TransportError;
use ouisync_lib::ShareToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    /// Mirror repository on a remote server
    Mirror { share_token: ShareToken },
}

#[derive(Serialize, Deserialize)]
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
