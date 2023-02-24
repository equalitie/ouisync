use crate::{
    directory::Directory,
    error::{ErrorCode, Result},
    network::NetworkEvent,
    registry::Handle,
};
use ouisync_lib::{PeerInfo, Progress, StateMonitor};
use serde::Serialize;
use serde_bytes::ByteBuf;
use std::net::SocketAddr;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerMessage {
    Success(Value),
    Failure { code: ErrorCode, message: String },
    Notification(Notification),
}

impl ServerMessage {
    pub fn response(result: Result<Value>) -> Self {
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

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Value {
    None,
    Bool(bool),
    U8(u8),
    U32(u32),
    U64(u64),
    Bytes(ByteBuf),
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

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(ByteBuf::from(value))
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
#[serde(untagged)]
pub(crate) enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}
