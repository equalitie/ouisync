use super::{ProtocolError, Response};
use ouisync_bridge::protocol::NetworkEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerPayload {
    Success(Response),
    Failure(ProtocolError),
    Notification(Notification),
}

impl From<Result<Response, ProtocolError>> for ServerPayload {
    fn from(result: Result<Response, ProtocolError>) -> Self {
        match result {
            Ok(response) => Self::Success(response),
            Err(error) => Self::Failure(error),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}
