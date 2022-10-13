use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum PeerSource {
    UserProvided,
    Listener,
    LocalDiscovery,
    Dht,
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
