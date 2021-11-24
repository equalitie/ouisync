use super::{
    message::Message,
    object_stream::{ObjectRead, ObjectWrite},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::{hash_map::Entry, HashMap},
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex as SyncMutex,
    },
};
use tokio::{
    net::tcp,
    select,
    sync::{mpsc, Mutex, Notify},
    task,
};

/// Wrapper for arbitrary number of `TcpObjectReader`s which reads from all of them simultaneously.
pub(super) struct MultiReader {
    tx: mpsc::Sender<Option<Message>>,
    // Wrapping these in Mutex and RwLock to have the `add` and `read` methods non mutable.  That
    // in turn is desirable to be able to call the two functions from different coroutines. Note
    // that we don't want to wrap this whole struct in a Mutex/RwLock because we don't want the add
    // function to be blocking.
    rx: Mutex<mpsc::Receiver<Option<Message>>>,
    count: AtomicUsize,
}

impl MultiReader {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(1);
        Self {
            tx,
            rx: Mutex::new(rx),
            count: AtomicUsize::new(0),
        }
    }

    pub fn add(&self, reader: tcp::OwnedReadHalf, permit: ConnectionPermitHalf) {
        let tx = self.tx.clone();

        // Using `SeqCst` here to be on the safe side although a weaker ordering would probably
        // suffice here (also in the `read` method).
        self.count.fetch_add(1, Ordering::SeqCst);

        task::spawn(async move {
            let mut reader = ObjectRead::new(reader);
            let _permit = permit; // make sure the permit is owned by this task.

            loop {
                select! {
                    result = reader.next() => {
                        if let Some(Ok(message)) = result {
                            tx.send(Some(message)).await.unwrap_or(())
                        } else {
                            tx.send(None).await.unwrap_or(());
                            break;
                        }
                    },
                    _ = tx.closed() => break,
                }
            }
        });
    }

    pub async fn read(&self) -> Option<Message> {
        loop {
            if self.count.load(Ordering::SeqCst) == 0 {
                return None;
            }

            match self.rx.lock().await.recv().await {
                Some(Some(message)) => return Some(message),
                Some(None) => {
                    self.count.fetch_sub(1, Ordering::SeqCst);
                }
                None => {
                    // This would mean that all senders were closed, but that can't happen because
                    // `self.tx` still exists.
                    unreachable!()
                }
            }
        }
    }
}

type WriterData = (
    Arc<Mutex<ObjectWrite<Message, tcp::OwnedWriteHalf>>>,
    ConnectionPermitHalf,
);

/// Wrapper for arbitrary number of `TcpObjectWriter`s which writes to the first available one.
pub(super) struct MultiWriter {
    // Using Mutexes and RwLocks here because we want the `add` and `write` functions to be const.
    // That will allow us to call them from two different coroutines. Note that we don't want this
    // whole structure to wrap because we don't want the `add` function to be blocking.
    next_id: AtomicUsize,
    writers: std::sync::RwLock<HashMap<usize, WriterData>>,
}

impl MultiWriter {
    pub fn new() -> Self {
        Self {
            next_id: AtomicUsize::new(0),
            writers: std::sync::RwLock::new(HashMap::new()),
        }
    }

    pub fn add(&self, writer: tcp::OwnedWriteHalf, permit: ConnectionPermitHalf) {
        // `Relaxed` ordering should be sufficient here because this is just a simple counter.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        self.writers
            .write()
            .unwrap()
            .insert(id, (Arc::new(Mutex::new(ObjectWrite::new(writer))), permit));
    }

    pub async fn write(&self, message: &Message) -> bool {
        while let Some((id, writer)) = self.pick_writer().await {
            if writer.lock().await.send(message).await.is_ok() {
                return true;
            }

            self.writers.write().unwrap().remove(&id);
        }

        false
    }

    async fn pick_writer(
        &self,
    ) -> Option<(usize, Arc<Mutex<ObjectWrite<Message, tcp::OwnedWriteHalf>>>)> {
        self.writers
            .read()
            .unwrap()
            .iter()
            .next()
            .map(|(id, (writer, _))| (*id, writer.clone()))
    }
}

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
            return None;
        };

        Some(ConnectionPermit {
            connections: self.connections.clone(),
            key,
            id,
            notify: Arc::new(Notify::new()),
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(super) enum ConnectionDirection {
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
