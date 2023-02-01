use crate::{
    directory::Directory,
    error::{ErrorCode, ToErrorCode},
    network::NetworkEvent,
    registry::Handle,
    request::Request,
};
use ouisync_lib::{PeerInfo, Progress, Result, StateMonitor};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Deserialize)]
pub(crate) struct ClientEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub message: Request,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ServerEnvelope {
    id: u64,
    #[serde(flatten)]
    message: ServerMessage,
}

impl ServerEnvelope {
    pub fn response(id: u64, result: Result<Value>) -> Self {
        let response = match result {
            Ok(response) => Response::Success(response),
            Err(error) => Response::Failure {
                code: error.to_error_code(),
                message: error.to_string(),
            },
        };

        Self {
            id,
            message: ServerMessage::Response(response),
        }
    }

    pub fn notification(id: u64, notification: Notification) -> Self {
        Self {
            id,
            message: ServerMessage::Notification(notification),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub(crate) enum ServerMessage {
    Response(Response),
    Notification(Notification),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Response {
    Success(Value),
    Failure { code: ErrorCode, message: String },
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Value {
    None,
    Bool(bool),
    U8(u8),
    U32(u32),
    Bytes(Vec<u8>),
    String(String),
    Handle(u64),
    Directory(Directory),
    StateMonitor(StateMonitor),
    Progress(Progress),
    PeerInfo(Vec<PeerInfo>),
}

impl<T> From<Option<T>> for Value
where
    Value: From<T>,
{
    fn from(value: Option<T>) -> Self {
        if let Some(value) = value {
            Self::from(value)
        } else {
            Self::None
        }
    }
}

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Self::U8(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::U32(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<StateMonitor> for Value {
    fn from(value: StateMonitor) -> Self {
        Self::StateMonitor(value)
    }
}

impl From<Directory> for Value {
    fn from(value: Directory) -> Self {
        Self::Directory(value)
    }
}

impl<T> From<Handle<T>> for Value {
    fn from(value: Handle<T>) -> Self {
        Self::Handle(value.id())
    }
}

impl From<SocketAddr> for Value {
    fn from(value: SocketAddr) -> Self {
        Self::String(value.to_string())
    }
}

impl From<Progress> for Value {
    fn from(value: Progress) -> Self {
        Self::Progress(value)
    }
}

impl From<Vec<PeerInfo>> for Value {
    fn from(value: Vec<PeerInfo>) -> Self {
        Self::PeerInfo(value)
    }
}

#[derive(Serialize)]
pub(crate) enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}
