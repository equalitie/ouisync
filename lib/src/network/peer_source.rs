use num_enum::{IntoPrimitive, TryFromPrimitive};
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
pub enum PeerSource {
    /// Explicitly added by the user.
    UserProvided,
    /// Peer connected to us.
    Listener,
    /// Discovered on the Local Discovery.
    LocalDiscovery,
    /// Discovered on the DHT.
    Dht,
    /// Discovered on the Peer Exchange.
    PeerExchange,
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
