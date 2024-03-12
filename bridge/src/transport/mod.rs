pub mod tls;

mod remote;
mod socket;

pub use self::{
    remote::{make_client_config, make_server_config, RemoteClient, RemoteServer},
    socket::{server_connection as socket_server_connection, SocketClient},
};

use crate::protocol::{Notification, SessionCookie};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

#[async_trait]
pub trait Handler: Clone + Send + Sync + 'static {
    type Request: DeserializeOwned + Send;
    type Response: Serialize + Send;
    type Error: From<TransportError> + Serialize + Send;

    async fn handle(
        &self,
        request: Self::Request,
        context: &SessionContext,
    ) -> Result<Self::Response, Self::Error>;
}

/// Context associated with a given client session.
pub struct SessionContext {
    pub notification_tx: NotificationSender,
    pub session_cookie: SessionCookie,
}

pub type NotificationSender = mpsc::Sender<(u64, Notification)>;

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("malformed message")]
pub enum TransportError {
    #[error("connection lost")]
    ConnectionLost,
    #[error("malformed message")]
    MalformedMessage,
}
