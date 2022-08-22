use super::{peer_addr::PeerAddr, peer_source::PeerSource};
use serde::{Serialize, Serializer};
use std::{
    collections::{hash_map::Entry, HashMap},
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as SyncMutex, Weak,
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
    connections: Arc<SyncMutex<HashMap<ConnectionKey, Peer>>>,
    on_change_tx: Arc<uninitialized_watch::Sender<()>>,
    // We need to keep this to prevent the Sender from closing.
    on_change_rx: uninitialized_watch::Receiver<()>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        let (tx, rx) = uninitialized_watch::channel();

        Self {
            next_id: AtomicU64::new(0),
            connections: Arc::new(SyncMutex::new(HashMap::new())),
            on_change_tx: Arc::new(tx),
            on_change_rx: rx,
        }
    }

    /// Attempt to reserve an connection to the given peer. If the connection hasn't been reserved
    /// yet, it returns a `ConnectionPermit` which keeps the connection reserved as long as it
    /// lives. Otherwise it returns `None`. To release a connection the permit needs to be dropped.
    /// Also returns a notification object that can be used to wait until the permit gets released.
    pub fn reserve(
        &self,
        addr: PeerAddr,
        source: PeerSource,
        dir: ConnectionDirection,
    ) -> ReserveResult {
        let key = ConnectionKey { addr, dir };
        match self.connections.lock().unwrap().entry(key) {
            Entry::Vacant(entry) => {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                let on_release = Arc::new(DropAwaitable::new());
                let on_release_weak = Arc::downgrade(&on_release);

                entry.insert(Peer {
                    id,
                    state: PeerState::Known,
                    source,
                    on_release,
                });
                self.on_change_tx.send(()).unwrap_or(());

                ReserveResult::Permit(ConnectionPermit {
                    connections: self.connections.clone(),
                    key,
                    id,
                    on_release: on_release_weak,
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
        self.on_change_rx.clone()
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

#[derive(Clone)]
struct Peer {
    id: u64,
    state: PeerState,
    source: PeerSource,
    on_release: Arc<DropAwaitable>,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize)]
pub enum ConnectionDirection {
    Incoming,
    Outgoing,
}

/// Connection permit that prevents another connection to the same peer (socket address) to be
/// established as long as it remains in scope.
pub(super) struct ConnectionPermit {
    connections: Arc<SyncMutex<HashMap<ConnectionKey, Peer>>>,
    key: ConnectionKey,
    id: u64,
    on_release: Weak<DropAwaitable>,
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
                key: self.key,
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
        let peer = lock.get_mut(&self.key).unwrap();

        if peer.state != new_state {
            peer.state = new_state;
            self.on_deduplicator_change.send(()).unwrap_or(());
        }
    }

    /// Returns a `Notify` that gets notified when this permit gets released.
    pub fn released(&self) -> AwaitDrop {
        // Unwrap is OK because the entry still exists in the map until Self is dropped.
        // (With one edge case when `self` is a `ConnectionPermit` inside `ConnectionPermitHalf`,
        // but `ConnectionPermitHalf` does not expose this `released` fn, so we should be good).
        self.on_release.upgrade().unwrap().subscribe()
    }

    pub fn addr(&self) -> PeerAddr {
        self.key.addr
    }

    /// Dummy connection permit for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        use std::net::Ipv4Addr;
        let on_release = Arc::new(DropAwaitable::new());

        Self {
            connections: Arc::new(SyncMutex::new(HashMap::new())),
            key: ConnectionKey {
                addr: PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 0).into()),
                dir: ConnectionDirection::Incoming,
            },
            id: 0,
            on_release: Arc::downgrade(&on_release),
            on_deduplicator_change: Arc::new(uninitialized_watch::channel().0),
        }
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        if let Entry::Occupied(entry) = self.connections.lock().unwrap().entry(self.key) {
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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConnectionKey {
    pub addr: PeerAddr,
    pub dir: ConnectionDirection,
}
