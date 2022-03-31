use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as SyncMutex,
    },
};
use tokio::sync::Notify;

/// Prevents establishing duplicate connections.
pub(super) struct ConnectionDeduplicator {
    next_id: AtomicU64,
    connections: Arc<SyncMutex<HashMap<ConnectionKey, u64>>>,
}

impl ConnectionDeduplicator {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            connections: Arc::new(SyncMutex::new(HashMap::new())),
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
            entry.insert(id);
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
            notify: Arc::new(Notify::new()),
        })
    }

    pub fn collect_peer_info(&self) -> HashMap<SocketAddr, HashSet<ConnectionDirection>> {
        let connections = self.connections.lock().unwrap();

        let mut map = HashMap::<_, HashSet<_>>::new();

        for c in connections.keys() {
            match map.entry(c.addr) {
                Entry::Vacant(entry) => {
                    entry.insert(std::iter::once(c.dir).collect());
                },
                Entry::Occupied(mut entry) => {
                    entry.get_mut().insert(c.dir);
                }
            }
        }

        map
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum ConnectionDirection {
    Incoming,
    Outgoing,
}

/// Connection permit that prevents another connection to the same peer (socket address) to be
/// established as long as it remains in scope.
pub(super) struct ConnectionPermit {
    connections: Arc<SyncMutex<HashMap<ConnectionKey, u64>>>,
    key: ConnectionKey,
    id: u64,
    notify: Arc<Notify>,
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
                notify: self.notify.clone(),
            }),
            ConnectionPermitHalf(self),
        )
    }

    /// Returns a `Notify` that gets notified when this permit gets released.
    pub fn released(&self) -> Arc<Notify> {
        self.notify.clone()
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
                dir: ConnectionDirection::Incoming,
                addr: (Ipv4Addr::UNSPECIFIED, 0).into(),
            },
            id: 0,
            notify: Arc::new(Notify::new()),
        }
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        if let Entry::Occupied(entry) = self.connections.lock().unwrap().entry(self.key) {
            if *entry.get() == self.id {
                entry.remove();
            }
        }

        self.notify.notify_one()
    }
}

/// Half of a connection permit. Dropping it drops the whole permit.
/// See [`ConnectionPermit::split`] for more details.
pub(super) struct ConnectionPermitHalf(ConnectionPermit);

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct ConnectionKey {
    dir: ConnectionDirection,
    addr: SocketAddr,
}
