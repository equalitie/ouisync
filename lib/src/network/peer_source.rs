use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_macros::api;
use serde::{Deserialize, Serialize};
use std::fmt;

/// How was the peer discovered.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    IntoPrimitive,
    TryFromPrimitive,
)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
#[api]
pub enum PeerSource {
    /// Explicitly added by the user.
    UserProvided = 0,
    /// Peer connected to us.
    Listener = 1,
    /// Discovered on the Local Discovery.
    LocalDiscovery = 2,
    /// Discovered on the DHT.
    Dht = 3,
    /// Discovered on the Peer Exchange.
    PeerExchange = 4,
}

impl PeerSource {
    pub(super) fn direction(&self) -> ConnectionDirection {
        match self {
            Self::Listener => ConnectionDirection::Incoming,
            Self::UserProvided | Self::LocalDiscovery | Self::Dht | Self::PeerExchange => {
                ConnectionDirection::Outgoing
            }
        }
    }
}

impl fmt::Display for PeerSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PeerSource::Listener => write!(f, "incoming"),
            PeerSource::UserProvided => write!(f, "outgoing (user provided)"),
            PeerSource::LocalDiscovery => write!(f, "outgoing (locally discovered)"),
            PeerSource::Dht => write!(f, "outgoing (found on DHT)"),
            PeerSource::PeerExchange => write!(f, "outgoing (found on peer exchange)"),
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub(super) enum ConnectionDirection {
    /// Peer connected to us
    Incoming,
    /// We connected to the peer
    Outgoing,
}

impl ConnectionDirection {
    pub fn glyph(&self) -> char {
        match self {
            Self::Incoming => '↓',
            Self::Outgoing => '↑',
        }
    }
}
