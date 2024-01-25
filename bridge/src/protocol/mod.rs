pub mod remote;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServerMessage<T, E> {
    Success(T),
    Failure(E),
    Notification(Notification),
}

impl<T, E> ServerMessage<T, E> {
    pub fn response(result: Result<T, E>) -> Self {
        match result {
            Ok(response) => Self::Success(response),
            Err(error) => Self::Failure(error),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_message_serialize_deserialize() {
        #[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
        enum TestResponse {
            None,
            Bool(bool),
        }

        #[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]
        enum TestError {
            ForbiddenRequest,
            Io,
        }

        let origs = [
            ServerMessage::response(Ok(TestResponse::None)),
            ServerMessage::response(Ok(TestResponse::Bool(true))),
            ServerMessage::response(Ok(TestResponse::Bool(false))),
            ServerMessage::response(Err(TestError::ForbiddenRequest)),
            ServerMessage::response(Err(TestError::Io)),
        ];

        for orig in origs {
            let encoded = rmp_serde::to_vec(&orig).unwrap();
            println!("{encoded:?}");
            let decoded: ServerMessage<TestResponse, TestError> =
                rmp_serde::from_slice(&encoded).unwrap();
            assert_eq!(decoded, orig);
        }
    }
}
