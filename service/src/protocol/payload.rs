use super::{ProtocolError, Response};
use num_enum::{IntoPrimitive, TryFromPrimitive};
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

/// Network notification event.
#[derive(
    Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize, TryFromPrimitive, IntoPrimitive,
)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum NetworkEvent {
    /// A peer has appeared with higher protocol version than us. Probably means we are using
    /// outdated library. This event can be used to notify the user that they should update the app.
    ProtocolVersionMismatch = 0,
    /// The set of known peers has changed (e.g., a new peer has been discovered)
    PeerSetChange = 1,
}
