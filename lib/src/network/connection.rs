use super::{peer_addr::PeerAddr, peer_source::PeerSource, runtime_id::PublicRuntimeId};
use crate::{
    collections::{hash_map::Entry, HashMap},
    deadlock::BlockingMutex,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

// NOTE: Watch (or uninitialized_watch) has an advantage over Notify in that it's better at
// broadcasting to multiple consumers. This particular line is problematic in Notify documentation:
//
//   https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html#method.notify_waiters
//
//   "Unlike with notify_one(), no permit is stored to be used by the next call to notified().await."
//
// The issue is described in more detail here:
//
//   https://github.com/tokio-rs/tokio/issues/3757
use crate::sync::{uninitialized_watch, AwaitDrop, DropAwaitable};

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
pub enum PeerState {
    Known,
    Connecting,
    Handshaking,
    Active(PublicRuntimeId),
}

/// Prevents establishing duplicate connections.
pub(super) struct ConnectionDeduplicator {
    next_id: AtomicU64,
    connections: Arc<BlockingMutex<HashMap<ConnectionInfo, Peer>>>,
    on_change_tx: Arc<uninitialized_watch::Sender<()>>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        let (tx, _) = uninitialized_watch::channel();

        Self {
            next_id: AtomicU64::new(0),
            connections: Arc::new(BlockingMutex::new(HashMap::default())),
            on_change_tx: Arc::new(tx),
        }
    }

    /// Attempt to reserve an connection to the given peer. If the connection hasn't been reserved
    /// yet, it returns a `ConnectionPermit` which keeps the connection reserved as long as it
    /// lives. Otherwise it returns `None`. To release a connection the permit needs to be dropped.
    /// Also returns a notification object that can be used to wait until the permit gets released.
    pub fn reserve(&self, addr: PeerAddr, source: PeerSource) -> ReserveResult {
        let info = ConnectionInfo {
            addr,
            dir: ConnectionDirection::from_source(source),
        };

        match self.connections.lock().unwrap().entry(info) {
            Entry::Vacant(entry) => {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let on_release_tx = DropAwaitable::new();
                let on_release_rx = on_release_tx.subscribe();

                entry.insert(Peer {
                    id,
                    state: PeerState::Known,
                    source,
                    on_release: on_release_tx,
                });
                self.on_change_tx.send(()).unwrap_or(());

                ReserveResult::Permit(ConnectionPermit {
                    connections: self.connections.clone(),
                    info,
                    id,
                    on_release: on_release_rx,
                    on_deduplicator_change: self.on_change_tx.clone(),
                })
            }
            Entry::Occupied(entry) => {
                ReserveResult::Occupied(entry.get().on_release.subscribe(), entry.get().source)
            }
        }
    }

    // Sorted by the IP, so the user sees similar IPs together.
    pub fn collect_peer_info(&self) -> Vec<PeerInfo> {
        self.connections
            .lock()
            .unwrap()
            .iter()
            .map(|(key, peer)| PeerInfo::new(&key.addr, peer))
            .collect()
    }

    pub fn get_peer_info(&self, addr: PeerAddr) -> Option<PeerInfo> {
        let connections = self.connections.lock().unwrap();

        let incoming = ConnectionInfo {
            addr,
            dir: ConnectionDirection::Incoming,
        };
        let outgoing = ConnectionInfo {
            addr,
            dir: ConnectionDirection::Outgoing,
        };

        connections
            .get(&incoming)
            .or_else(|| connections.get(&outgoing))
            .map(|peer| PeerInfo::new(&addr, peer))
    }

    pub fn on_change(&self) -> uninitialized_watch::Receiver<()> {
        self.on_change_tx.subscribe()
    }
}

pub(super) enum ReserveResult {
    Permit(ConnectionPermit),
    // Use the receiver to get notified when the existing permit is destroyed.
    Occupied(AwaitDrop, PeerSource),
}

/// Information about a peer.
#[derive(Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PeerInfo {
    #[serde(with = "as_str")]
    pub ip: IpAddr,
    pub port: u16,
    pub source: PeerSource,
    pub state: PeerState,
}

impl PeerInfo {
    fn new(addr: &PeerAddr, peer: &Peer) -> Self {
        Self {
            ip: addr.socket_addr().ip(),
            port: addr.socket_addr().port(),
            source: peer.source,
            state: peer.state,
        }
    }
}

mod as_str {
    use super::*;

    pub fn serialize<S>(value: &IpAddr, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.to_string().serialize(s)
    }

    pub fn deserialize<'de, D>(d: D) -> Result<IpAddr, D::Error>
    where
        D: Deserializer<'de>,
    {
        <&str>::deserialize(d)?.parse().map_err(D::Error::custom)
    }
}

struct Peer {
    id: u64,
    state: PeerState,
    source: PeerSource,
    on_release: DropAwaitable,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub enum ConnectionDirection {
    Incoming,
    Outgoing,
}

impl ConnectionDirection {
    fn from_source(source: PeerSource) -> Self {
        match source {
            PeerSource::Listener => Self::Incoming,
            PeerSource::UserProvided
            | PeerSource::LocalDiscovery
            | PeerSource::Dht
            | PeerSource::PeerExchange => Self::Outgoing,
        }
    }
}

/// Connection permit that prevents another connection to the same peer (socket address) to be
/// established as long as it remains in scope.
pub(super) struct ConnectionPermit {
    connections: Arc<BlockingMutex<HashMap<ConnectionInfo, Peer>>>,
    info: ConnectionInfo,
    id: u64,
    on_release: AwaitDrop,
    on_deduplicator_change: Arc<uninitialized_watch::Sender<()>>,
}

impl ConnectionPermit {
    /// Split the permit into two halves where dropping any of them releases the whole permit.
    /// This is useful when the connection needs to be split into a reader and a writer Then if any
    /// of them closes, the whole connection closes. So both the reader and the writer should be
    /// associated with one half of the permit so that when any of them closes, the permit is
    /// released.
    pub fn split(self) -> (ConnectionPermitHalf, ConnectionPermitHalf) {
        (
            ConnectionPermitHalf(Self {
                connections: self.connections.clone(),
                info: self.info,
                id: self.id,
                on_release: self.on_release.clone(),
                on_deduplicator_change: self.on_deduplicator_change.clone(),
            }),
            ConnectionPermitHalf(self),
        )
    }

    pub fn mark_as_connecting(&self) {
        self.set_state(PeerState::Connecting);
    }

    pub fn mark_as_handshaking(&self) {
        self.set_state(PeerState::Handshaking);
    }

    pub fn mark_as_active(&self, runtime_id: PublicRuntimeId) {
        self.set_state(PeerState::Active(runtime_id));
    }

    fn set_state(&self, new_state: PeerState) {
        let mut lock = self.connections.lock().unwrap();

        // unwrap is ok because if `self` exists then the entry should exists as well.
        let peer = lock.get_mut(&self.info).unwrap();

        if peer.state != new_state {
            peer.state = new_state;
            self.on_deduplicator_change.send(()).unwrap_or(());
        }
    }

    /// Returns a `AwaitDrop` that gets notified when this permit gets released.
    pub fn released(&self) -> AwaitDrop {
        self.on_release.clone()
    }

    pub fn addr(&self) -> PeerAddr {
        self.info.addr
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn source(&self) -> PeerSource {
        self.connections
            .lock()
            .unwrap()
            .get(&self.info)
            // unwrap is ok because if `self` exists then the entry should exists as well.
            .unwrap()
            .source
    }

    /// Dummy connection permit for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        use std::net::Ipv4Addr;
        let on_release = DropAwaitable::new().subscribe();

        Self {
            connections: Arc::new(BlockingMutex::new(HashMap::default())),
            info: ConnectionInfo {
                addr: PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 0).into()),
                dir: ConnectionDirection::Incoming,
            },
            id: 0,
            on_release,
            on_deduplicator_change: Arc::new(uninitialized_watch::channel().0),
        }
    }
}

impl fmt::Debug for ConnectionPermit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ConnectionPermit(id:{}, {:?})", self.id, self.info)
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        if let Entry::Occupied(entry) = self.connections.lock().unwrap().entry(self.info) {
            if entry.get().id == self.id {
                entry.remove();
            }
        }

        self.on_deduplicator_change.send(()).unwrap_or(());
    }
}

/// Half of a connection permit. Dropping it drops the whole permit.
/// See [`ConnectionPermit::split`] for more details.
pub(super) struct ConnectionPermitHalf(ConnectionPermit);

impl ConnectionPermitHalf {
    pub fn info(&self) -> ConnectionInfo {
        self.0.info
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConnectionInfo {
    pub addr: PeerAddr,
    pub dir: ConnectionDirection,
}
