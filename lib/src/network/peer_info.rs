use super::{peer_addr::PeerAddr, peer_source::PeerSource, peer_state::PeerState, stats::Stats};
use ouisync_macros::api;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

/// Information about a peer.
#[derive(Eq, PartialEq, Serialize, Deserialize, Debug)]
#[api]
pub struct PeerInfo {
    #[serde(with = "as_str")]
    pub addr: PeerAddr,
    pub source: PeerSource,
    pub state: PeerState,
    pub stats: Stats,
}

mod as_str {
    use super::*;

    pub fn serialize<S>(value: &PeerAddr, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.to_string().serialize(s)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<PeerAddr, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}
