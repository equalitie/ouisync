use super::{
    peer_addr::PeerAddr,
    peer_info::PeerInfo,
    peer_source::PeerSource,
    peer_state::PeerState,
    runtime_id::PublicRuntimeId,
    stats::{ByteCounters, StatsTracker},
};
use crate::{
    collections::{hash_map::Entry, HashMap},
    sync::{AwaitDrop, DropAwaitable, WatchSenderExt},
};
use serde::Serialize;
use std::{
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::SystemTime,
};
use tokio::sync::watch;

/// Container for known connections.
pub(super) struct ConnectionSet {
    connections: watch::Sender<HashMap<Key, Data>>,
}

impl ConnectionSet {
    pub fn new() -> Self {
        Self {
            connections: watch::Sender::new(HashMap::default()),
        }
    }

    /// Attempt to reserve an connection to the given peer. If the connection hasn't been reserved
    /// yet, it returns a `ConnectionPermit` which keeps the connection reserved as long as it
    /// lives. Otherwise it returns `None`. To release a connection the permit needs to be dropped.
    /// Also returns a notification object that can be used to wait until the permit gets released.
    pub fn reserve(&self, addr: PeerAddr, source: PeerSource) -> ReserveResult {
        let key = Key {
            addr,
            dir: ConnectionDirection::from_source(source),
        };

        self.connections
            .send_if_modified_return(|connections| match connections.entry(key) {
                Entry::Vacant(entry) => {
                    let id = ConnectionId::next();

                    entry.insert(Data {
                        id,
                        state: PeerState::Known,
                        source,
                        stats_tracker: StatsTracker::default(),
                        on_release: DropAwaitable::new(),
                    });

                    (
                        true,
                        ReserveResult::Permit(ConnectionPermit {
                            connections: self.connections.clone(),
                            key,
                            id,
                        }),
                    )
                }
                Entry::Occupied(entry) => {
                    let peer_permit = entry.get();

                    (
                        false,
                        ReserveResult::Occupied(
                            peer_permit.on_release.subscribe(),
                            peer_permit.source,
                            peer_permit.id,
                        ),
                    )
                }
            })
    }

    pub fn peer_info_collector(&self) -> PeerInfoCollector {
        PeerInfoCollector(self.connections.clone())
    }

    pub fn get_peer_info(&self, addr: PeerAddr) -> Option<PeerInfo> {
        let connections = self.connections.borrow();

        connections
            .get(&Key {
                addr,
                dir: ConnectionDirection::Incoming,
            })
            .or_else(|| {
                connections.get(&Key {
                    addr,
                    dir: ConnectionDirection::Outgoing,
                })
            })
            .map(|data| data.peer_info(addr))
    }

    pub fn subscribe(&self) -> ConnectionSetSubscription {
        ConnectionSetSubscription(self.connections.subscribe())
    }
}

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

pub(super) enum ReserveResult {
    Permit(ConnectionPermit),
    // Use the receiver to get notified when the existing permit is destroyed.
    Occupied(AwaitDrop, PeerSource, ConnectionId),
}

#[derive(Clone)]
pub struct ConnectionSetSubscription(watch::Receiver<HashMap<Key, Data>>);

impl ConnectionSetSubscription {
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.0.changed().await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct PeerInfoCollector(watch::Sender<HashMap<Key, Data>>);

impl PeerInfoCollector {
    pub fn collect(&self) -> Vec<PeerInfo> {
        self.0
            .borrow()
            .iter()
            .map(|(key, data)| data.peer_info(key.addr))
            .collect()
    }
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
    connections: watch::Sender<HashMap<Key, Data>>,
    key: Key,
    id: ConnectionId,
}

impl ConnectionPermit {
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
        self.connections.send_if_modified(|connections| {
            // unwrap is ok because if `self` exists then the entry should exists as well.
            let peer = connections.get_mut(&self.key).unwrap();

            if peer.state != new_state {
                peer.state = new_state;
                true
            } else {
                false
            }
        });
    }

    /// Returns a `AwaitDrop` that gets notified when this permit gets released.
    pub fn released(&self) -> AwaitDrop {
        // We can't use unwrap here because this method is used in `ConnectionPermitHalf` which can
        // outlive the entry if the other half gets dropped.
        self.with(|data| data.on_release.subscribe())
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
        self.with(|data| data.source).unwrap()
    }

    pub fn byte_counters(&self) -> Arc<ByteCounters> {
        self.with(|data| data.stats_tracker.bytes.clone())
            .unwrap_or_default()
    }

    fn with<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&Data) -> R,
    {
        self.connections.borrow().get(&self.key).map(f)
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
        self.connections.send_if_modified(|connections| {
            let Entry::Occupied(entry) = connections.entry(self.key) else {
                return false;
            };

            if entry.get().id != self.id {
                return false;
            }

            entry.remove();
            true
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct Key {
    addr: PeerAddr,
    dir: ConnectionDirection,
}

struct Data {
    id: ConnectionId,
    state: PeerState,
    source: PeerSource,
    stats_tracker: StatsTracker,
    on_release: DropAwaitable,
}

impl Data {
    fn peer_info(&self, addr: PeerAddr) -> PeerInfo {
        let stats = self.stats_tracker.read();

        PeerInfo {
            addr,
            source: self.source,
            state: self.state,
            stats,
        }
    }
}
