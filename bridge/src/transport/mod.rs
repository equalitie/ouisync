mod remote;
mod socket;

pub use self::{
    remote::{make_client_config, make_server_config, RemoteClient, RemoteServer},
    socket::{server_connection as socket_server_connection, SocketClient},
};

use crate::protocol::Notification;
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
        notification_tx: &NotificationSender,
    ) -> Result<Self::Response, Self::Error>;
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
