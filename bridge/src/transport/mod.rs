mod remote;
mod socket;

pub use self::{
    remote::{make_client_config, make_server_config, RemoteClient, RemoteServer},
    socket::{server_connection as socket_server_connection, SocketClient},
};

use crate::{error::Result, protocol::Notification};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;

#[async_trait]
pub trait Handler: Clone + Send + Sync + 'static {
    type Request: DeserializeOwned + Send;
    type Response: Serialize + Send;

    async fn handle(
        &self,
        request: Self::Request,
        notification_tx: &NotificationSender,
    ) -> Result<Self::Response>;
}

pub type NotificationSender = mpsc::Sender<(u64, Notification)>;
