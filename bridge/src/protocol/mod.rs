pub mod remote;

use crate::{
    constants::{NETWORK_EVENT_PEER_SET_CHANGE, NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH},
    error::{Error, ErrorCode, Result, ToErrorCode},
};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};

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
                // TODO: include also sources
                message: match error {
                    Error::Io(inner) => inner.to_string(),
                    _ => error.to_string(),
                },
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

#[derive(
    Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize, TryFromPrimitive, IntoPrimitive,
)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum NetworkEvent {
    ProtocolVersionMismatch = NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH,
    PeerSetChange = NETWORK_EVENT_PEER_SET_CHANGE,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::io;

    #[test]
    fn server_message_serialize_deserialize() {
        #[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
        enum TestResponse {
            None,
            Bool(bool),
        }

        let origs = [
            ServerMessage::response(Ok(TestResponse::None)),
            ServerMessage::response(Ok(TestResponse::Bool(true))),
            ServerMessage::response(Ok(TestResponse::Bool(false))),
            ServerMessage::response(Err(Error::ForbiddenRequest)),
            ServerMessage::response(Err(Error::Io(io::Error::new(
                io::ErrorKind::Other,
                "something went wrong",
            )))),
        ];

        for orig in origs {
            let encoded = rmp_serde::to_vec(&orig).unwrap();
            println!("{encoded:?}");
            let decoded: ServerMessage<TestResponse> = rmp_serde::from_slice(&encoded).unwrap();
            assert_eq!(decoded, orig);
        }
    }
}
