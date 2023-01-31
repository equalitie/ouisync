use crate::{
    directory::Directory,
    error::{ErrorCode, ToErrorCode},
    network::NetworkEvent,
    registry::Handle,
    repository::RepositoryHolder,
    request::Request,
    session::SubscriptionHandle,
};
use ouisync_lib::{Result, StateMonitor};
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
    Bytes(Vec<u8>),
    String(String),
    Repository(Handle<RepositoryHolder>),
    Subscription(SubscriptionHandle),
    Directory(Directory),
    StateMonitor(StateMonitor),
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

impl From<Handle<RepositoryHolder>> for Value {
    fn from(value: Handle<RepositoryHolder>) -> Self {
        Self::Repository(value)
    }
}

impl From<SubscriptionHandle> for Value {
    fn from(value: SubscriptionHandle) -> Self {
        Self::Subscription(value)
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

impl From<SocketAddr> for Value {
    fn from(value: SocketAddr) -> Self {
        Self::String(value.to_string())
    }
}

#[derive(Serialize)]
pub(crate) enum Notification {
    Repository,
    Network(NetworkEvent),
    StateMonitor,
}
