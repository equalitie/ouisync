use std::{
    collections::{BTreeMap, hash_map::Entry, HashMap},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as SyncMutex,
    },
};
// NOTE: Watch has an advantage over Notify in that it's better at broadcasting to multiple
// consumers. This particular line is problematic in Notify documentation:
//
//   https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html#method.notify_waiters
//
//   "Unlike with notify_one(), no permit is stored to be used by the next call to notified().await."
//
// The issue is described in more detail here:
//
//   https://github.com/tokio-rs/tokio/issues/3757
use tokio::sync::{watch, Notify};

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
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
    on_change_tx: Arc<watch::Sender<bool>>,
    // We need to keep this to prevent the Sender from closing.
    on_change_rx: watch::Receiver<bool>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);

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
    pub fn reserve(&self, addr: SocketAddr, dir: ConnectionDirection) -> Option<ConnectionPermit> {
        let key = ConnectionKey { addr, dir };
        let id = if let Entry::Vacant(entry) = self.connections.lock().unwrap().entry(key) {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            entry.insert(Peer {
                id,
                state: PeerState::Known,
            });
            self.on_change_tx.send(true).unwrap_or(());
            id
        } else {
            log::debug!(
                "ConnectionDeduplicator: Not permitting another connection ({:?}, {:?})",
                addr,
                dir
            );
            return None;
        };

        Some(ConnectionPermit {
            connections: self.connections.clone(),
            key,
            id,
            on_release: Arc::new(Notify::new()),
            on_deduplicator_change: self.on_change_tx.clone(),
        })
    }

    // Using BTreeMap for the user to see similar IPs together.
    pub fn collect_peer_info(&self) -> BTreeMap<ConnectionKey, PeerState> {
        let connections = self.connections.lock().unwrap();
        connections
            .iter()
            .map(|(key, peer)| (*key, peer.state))
            .collect()
    }

    pub fn on_change(&self) -> watch::Receiver<bool> {
        self.on_change_rx.clone()
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct Peer {
    id: u64,
    state: PeerState,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
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
    on_release: Arc<Notify>,
    on_deduplicator_change: Arc<watch::Sender<bool>>,
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
            self.on_deduplicator_change.send(true).unwrap_or(());
        }
    }

    /// Returns a `Notify` that gets notified when this permit gets released.
    pub fn released(&self) -> Arc<Notify> {
        self.on_release.clone()
    }

    pub fn addr(&self) -> SocketAddr {
        self.key.addr
    }

    /// Dummy connection permit for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        use std::net::Ipv4Addr;

        Self {
            connections: Arc::new(SyncMutex::new(HashMap::new())),
            key: ConnectionKey {
                addr: (Ipv4Addr::UNSPECIFIED, 0).into(),
                dir: ConnectionDirection::Incoming,
            },
            id: 0,
            on_release: Arc::new(Notify::new()),
            on_deduplicator_change: Arc::new(watch::channel(false).0),
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

        self.on_release.notify_one();
        self.on_deduplicator_change.send(true).unwrap_or(());
    }
}

/// Half of a connection permit. Dropping it drops the whole permit.
/// See [`ConnectionPermit::split`] for more details.
pub(super) struct ConnectionPermitHalf(ConnectionPermit);

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConnectionKey {
    pub addr: SocketAddr,
    pub dir: ConnectionDirection,
}
