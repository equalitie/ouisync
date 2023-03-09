use crate::{
    constants::{NETWORK_EVENT_PEER_SET_CHANGE, NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH},
    error::{ErrorCode, Result},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerMessage<T> {
    Success(T),
    Failure { code: ErrorCode, message: String },
    Notification(Notification),
}

impl<T> ServerMessage<T> {
    pub fn response(result: Result<T>) -> Self {
        match result {
            Ok(response) => Self::Success(response),
            Err(error) => Self::Failure {
                code: error.to_error_code(),
                message: error.to_string(),
            },
        }
    }

    pub fn notification(notification: Notification) -> Self {
        Self::Notification(notification)
    }
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum NetworkEvent {
    ProtocolVersionMismatch = NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH,
    PeerSetChange = NETWORK_EVENT_PEER_SET_CHANGE,
}

impl From<NetworkEvent> for u8 {
    fn from(event: NetworkEvent) -> Self {
        event as u8
    }
}

impl TryFrom<u8> for NetworkEvent {
    type Error = NetworkEventDecodeError;

    fn try_from(input: u8) -> Result<Self, Self::Error> {
        match input {
            NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH => Ok(Self::ProtocolVersionMismatch),
            NETWORK_EVENT_PEER_SET_CHANGE => Ok(Self::PeerSetChange),
            _ => Err(NetworkEventDecodeError),
        }
    }
}

#[derive(Error, Debug)]
#[error("failed to decode network event")]
pub struct NetworkEventDecodeError;
