pub mod remote;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Deserializer, Serialize};

pub trait DeserializeVersioned<'de>: Sized {
    fn deserialize_versioned<D>(version: u64, d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>;
}

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
    /// The list of repositories in a session has changed.
    RepositoryListChanged,
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

/// Opaque, non-sensitive value unique to a particular client session and accessible to both the
/// client and the server. It's useful for constructing zero-knowledge proofs: the client can sign
/// this cookie with a private key and send the signature to the server in order to prove the
/// possession of the private key without revealing it. The cookie is unique per session which
/// makes this proof resistant to replay attacks. The value should be constructed by the underlying
/// transport. For example if TLS is used, one can use the [export_keying_material]
/// (https://docs.rs/rustls/0.22.0/rustls/struct.ConnectionCommon.html#method.export_keying_material)
/// function. If no such proofs are needed, one can use `SesssionCookie::DUMMY` instead.
#[derive(Clone, Copy)]
pub struct SessionCookie([u8; 32]);

impl SessionCookie {
    /// Dummy, non-unique value for non-secure transports.
    pub const DUMMY: Self = Self([0; 32]);
}

impl AsRef<[u8]> for SessionCookie {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl AsMut<[u8]> for SessionCookie {
    fn as_mut(&mut self) -> &mut [u8] {
        self.0.as_mut()
    }
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
