use super::{peer_addr::PeerAddr, peer_source::PeerSource};
use serde::{Serialize, Serializer};
use std::{
    collections::{hash_map::Entry, HashMap},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as SyncMutex,
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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub enum PeerState {
    Known,
    Connecting,
    Handshaking,
    Active,
}

/// Prevents establishing duplicate connections.
pub(super) struct ConnectionDeduplicator {
    next_id: AtomicU64,
    connections: Arc<SyncMutex<HashMap<ConnectionInfo, Peer>>>,
    on_change_tx: Arc<uninitialized_watch::Sender<()>>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        let (tx, _) = uninitialized_watch::channel();

        Self {
            next_id: AtomicU64::new(0),
            connections: Arc::new(SyncMutex::new(HashMap::new())),
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
        let connections = self.connections.lock().unwrap();
        let mut infos: Vec<_> = connections
            .iter()
            .map(|(key, peer)| PeerInfo {
                ip: key.addr.ip(),
                port: key.addr.port(),
                direction: key.dir,
                state: peer.state,
            })
            .collect();
        infos.sort();
        infos
    }

    pub fn on_change(&self) -> uninitialized_watch::Receiver<()> {
        self.on_change_tx.subscribe()
    }

    pub fn is_connected_to(&self, addr: PeerAddr) -> bool {
        let connections = self.connections.lock().unwrap();
        let incoming = ConnectionInfo {
            addr,
            dir: ConnectionDirection::Incoming,
        };
        let outgoing = ConnectionInfo {
            addr,
            dir: ConnectionDirection::Outgoing,
        };

        connections.contains_key(&incoming) || connections.contains_key(&outgoing)
    }
}

pub(super) enum ReserveResult {
    Permit(ConnectionPermit),
    // Use the receiver to get notified when the existing permit is destroyed.
    Occupied(AwaitDrop, PeerSource),
}

/// Information about a peer.
#[derive(Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct PeerInfo {
    #[serde(serialize_with = "serialize_ip_as_string")]
    pub ip: IpAddr,
    pub port: u16,
    pub direction: ConnectionDirection,
    pub state: PeerState,
}

fn serialize_ip_as_string<S>(ip: &IpAddr, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    ip.to_string().serialize(s)
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
    connections: Arc<SyncMutex<HashMap<ConnectionInfo, Peer>>>,
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

    pub fn mark_as_active(&self) {
        self.set_state(PeerState::Active);
    }

    fn set_state(&self, new_state: PeerState) {
        let mut lock = self.connections.lock().unwrap();

        // Unwrap because if `self` exists, then the entry should exist as well.
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

    /// Dummy connection permit for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        use std::net::Ipv4Addr;
        let on_release = DropAwaitable::new().subscribe();

        Self {
            connections: Arc::new(SyncMutex::new(HashMap::new())),
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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConnectionInfo {
    pub addr: PeerAddr,
    pub dir: ConnectionDirection,
}
