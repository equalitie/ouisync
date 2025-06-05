use crate::collections::HashMap;
use futures_util::{stream, Stream, StreamExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{select, sync::watch};

use super::{
    connection::{ConnectionData, ConnectionKey},
    protocol::{Version, VERSION},
};

pub(super) struct ProtocolVersions {
    pub our: Version,
    pub highest_seen: Version,
}

impl ProtocolVersions {
    pub fn new() -> Self {
        Self {
            our: VERSION,
            highest_seen: Version::ZERO,
        }
    }
}

/// Network notification event.
#[derive(
    Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize, TryFromPrimitive, IntoPrimitive,
)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
#[api]
pub enum NetworkEvent {
    /// A peer has appeared with higher protocol version than us. Probably means we are using
    /// outdated library. This event can be used to notify the user that they should update the app.
    ProtocolVersionMismatch = 0,
    /// The set of known peers has changed (e.g., a new peer has been discovered)
    PeerSetChange = 1,
}

pub struct NetworkEventReceiver {
    protocol_versions: watch::Receiver<ProtocolVersions>,
    connections: watch::Receiver<HashMap<ConnectionKey, ConnectionData>>,
}

impl NetworkEventReceiver {
    pub(super) fn new(
        protocol_versions: watch::Receiver<ProtocolVersions>,
        connections: watch::Receiver<HashMap<ConnectionKey, ConnectionData>>,
    ) -> Self {
        Self {
            protocol_versions,
            connections,
        }
    }

    pub async fn recv(&mut self) -> Option<NetworkEvent> {
        select! {
            Ok(_) = self.protocol_versions.wait_for(|versions| versions.highest_seen > versions.our) => {
                Some(NetworkEvent::ProtocolVersionMismatch)
            }
            Ok(_) = self.connections.changed() => Some(NetworkEvent::PeerSetChange),
            else => None,
        }
    }
}

/// Wrapper around `NetworkEventReceiver` that implements `Stream`.
pub struct NetworkEventStream {
    inner: Pin<Box<dyn Stream<Item = NetworkEvent> + Send + 'static>>,
}

impl NetworkEventStream {
    pub fn new(rx: NetworkEventReceiver) -> Self {
        Self {
            inner: Box::pin(stream::unfold(rx, |mut rx| async move {
                Some((rx.recv().await?, rx))
            })),
        }
    }
}

impl Stream for NetworkEventStream {
    type Item = NetworkEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().inner.poll_next_unpin(cx)
    }
}
