use super::{
    peer_addr::PeerAddr, peer_info::PeerInfo, peer_source::PeerSource, peer_state::PeerState,
    runtime_id::PublicRuntimeId, traffic_tracker::TrafficTracker,
};
use crate::collections::{hash_map::Entry, HashMap};
use deadlock::BlockingMutex;
use serde::Serialize;
use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::SystemTime,
};

/// Unique identifier of a connection. Connections are mostly already identified by the peer address
/// and direction (incoming / outgoing), but this type allows to distinguish even connections with
/// the same address/direction but that were established in two separate occasions.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub(super) struct ConnectionId(u64);

impl ConnectionId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

/// Prevents establishing duplicate connections.
pub(super) struct ConnectionDeduplicator {
    connections: Arc<BlockingMutex<HashMap<ConnectionKey, Peer>>>,
    on_change_tx: uninitialized_watch::Sender<()>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        let (on_change_tx, _) = uninitialized_watch::channel();

        Self {
            connections: Arc::new(BlockingMutex::new(HashMap::default())),
            on_change_tx,
        }
    }

    /// Attempt to reserve an connection to the given peer. If the connection hasn't been reserved
    /// yet, it returns a `ConnectionPermit` which keeps the connection reserved as long as it
    /// lives. Otherwise it returns `None`. To release a connection the permit needs to be dropped.
    /// Also returns a notification object that can be used to wait until the permit gets released.
    pub fn reserve(&self, addr: PeerAddr, source: PeerSource) -> ReserveResult {
        let key = ConnectionKey {
            addr,
            dir: ConnectionDirection::from_source(source),
        };

        match self.connections.lock().unwrap().entry(key) {
            Entry::Vacant(entry) => {
                let id = ConnectionId::next();
                let on_release_tx = DropAwaitable::new();

                entry.insert(Peer {
                    id,
                    state: PeerState::Known,
                    source,
                    tracker: TrafficTracker::new(),
                    on_release: on_release_tx,
                });
                self.on_change_tx.send(()).unwrap_or(());

                ReserveResult::Permit(ConnectionPermit {
                    connections: self.connections.clone(),
                    key,
                    id,
                    on_deduplicator_change: self.on_change_tx.clone(),
                })
            }
            Entry::Occupied(entry) => {
                let peer_permit = entry.get();
                ReserveResult::Occupied(
                    peer_permit.on_release.subscribe(),
                    peer_permit.source,
                    peer_permit.id,
                )
            }
        }
    }

    pub fn peer_info_collector(&self) -> PeerInfoCollector {
        PeerInfoCollector(self.connections.clone())
    }

    pub fn get_peer_info(&self, addr: PeerAddr) -> Option<PeerInfo> {
        let connections = self.connections.lock().unwrap();

        let incoming = ConnectionKey {
            addr,
            dir: ConnectionDirection::Incoming,
        };
        let outgoing = ConnectionKey {
            addr,
            dir: ConnectionDirection::Outgoing,
        };

        connections
            .get(&incoming)
            .or_else(|| connections.get(&outgoing))
            .map(|peer| PeerInfo {
                addr,
                source: peer.source,
                state: peer.state,
                stats: peer.tracker.get(),
            })
    }

    pub fn on_change(&self) -> uninitialized_watch::Receiver<()> {
        self.on_change_tx.subscribe()
    }
}

pub(super) enum ReserveResult {
    Permit(ConnectionPermit),
    // Use the receiver to get notified when the existing permit is destroyed.
    Occupied(AwaitDrop, PeerSource, ConnectionId),
}

#[derive(Clone)]
pub struct PeerInfoCollector(Arc<BlockingMutex<HashMap<ConnectionKey, Peer>>>);

impl PeerInfoCollector {
    pub fn collect(&self) -> Vec<PeerInfo> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(key, peer)| PeerInfo {
                addr: key.addr,
                source: peer.source,
                state: peer.state,
                stats: peer.tracker.get(),
            })
            .collect()
    }
}

pub(super) struct Peer {
    id: ConnectionId,
    state: PeerState,
    source: PeerSource,
    tracker: TrafficTracker,
    on_release: DropAwaitable,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub(super) enum ConnectionDirection {
    Incoming,
    Outgoing,
}

impl ConnectionDirection {
    pub fn from_source(source: PeerSource) -> Self {
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
    connections: Arc<BlockingMutex<HashMap<ConnectionKey, Peer>>>,
    key: ConnectionKey,
    id: ConnectionId,
    on_deduplicator_change: uninitialized_watch::Sender<()>,
}

impl ConnectionPermit {
    /// Split the permit into two halves where dropping any of them releases the whole permit.
    /// This is useful when the connection needs to be split into a reader and a writer Then if any
    /// of them closes, the whole connection closes. So both the reader and the writer should be
    /// associated with one half of the permit so that when any of them closes, the permit is
    /// released.
    pub fn into_split(self) -> (ConnectionPermitHalf, ConnectionPermitHalf) {
        (
            ConnectionPermitHalf(Self {
                connections: self.connections.clone(),
                key: self.key,
                id: self.id,
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
        self.set_state(PeerState::Active {
            id: runtime_id,
            since: SystemTime::now(),
        });
    }

    fn set_state(&self, new_state: PeerState) {
        let mut lock = self.connections.lock().unwrap();

        // unwrap is ok because if `self` exists then the entry should exists as well.
        let peer = lock.get_mut(&self.key).unwrap();

        if peer.state != new_state {
            peer.state = new_state;
            self.on_deduplicator_change.send(()).unwrap_or(());
        }
    }

    /// Returns a `AwaitDrop` that gets notified when this permit gets released.
    pub fn released(&self) -> AwaitDrop {
        // We can't use unwrap here because this method is used in `ConnectionPermitHalf` which can
        // outlive the entry if the other half gets dropped.
        self.with_peer(|peer| peer.on_release.subscribe())
            .unwrap_or_else(|| DropAwaitable::new().subscribe())
    }

    pub fn addr(&self) -> PeerAddr {
        self.key.addr
    }

    pub fn id(&self) -> ConnectionId {
        self.id
    }

    pub fn source(&self) -> PeerSource {
        // unwrap is ok because if `self` exists then the entry should exists as well.
        self.with_peer(|peer| peer.source).unwrap()
    }

    /// Dummy connection permit for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        use std::net::Ipv4Addr;

        let key = ConnectionKey {
            addr: PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 0).into()),
            dir: ConnectionDirection::Incoming,
        };
        let id = ConnectionId::next();
        let peer = Peer {
            id,
            state: PeerState::Known,
            source: PeerSource::UserProvided,
            tracker: TrafficTracker::new(),
            on_release: DropAwaitable::new(),
        };

        Self {
            connections: Arc::new(BlockingMutex::new([(key, peer)].into())),
            key,
            id,
            on_deduplicator_change: uninitialized_watch::channel().0,
        }
    }

    fn with_peer<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Peer) -> R,
    {
        self.connections.lock().unwrap().get(&self.key).map(f)
    }
}

impl fmt::Debug for ConnectionPermit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ConnectionPermit")
            .field("key", &self.key)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let Ok(mut connections) = self.connections.lock() else {
            return;
        };

        let Entry::Occupied(entry) = connections.entry(self.key) else {
            return;
        };

        if entry.get().id != self.id {
            return;
        }

        entry.remove();
        self.on_deduplicator_change.send(()).ok();
    }
}

/// Half of a connection permit. Dropping it drops the whole permit.
/// See [`ConnectionPermit::split`] for more details.
pub(super) struct ConnectionPermitHalf(ConnectionPermit);

impl ConnectionPermitHalf {
    pub fn id(&self) -> ConnectionId {
        self.0.id
    }

    pub fn tracker(&self) -> TrafficTracker {
        self.0
            .with_peer(|peer| peer.tracker.clone())
            .unwrap_or_default()
    }

    pub fn released(&self) -> AwaitDrop {
        self.0.released()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct ConnectionKey {
    pub addr: PeerAddr,
    pub dir: ConnectionDirection,
}
