mod client;
mod message;
mod message_broker;
mod object_stream;
mod replica_discovery;
mod server;
#[cfg(test)]
mod tests;

use self::{
    message::RepositoryId,
    message_broker::MessageBroker,
    object_stream::TcpObjectStream,
    replica_discovery::{ReplicaDiscovery, RuntimeId},
};
use crate::{
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
    repository::Repository,
    scoped_task::{ScopedTaskHandle, ScopedTaskSet},
    tagged::{Local, Remote},
    upnp,
};
use async_recursion::async_recursion;
use futures_util::future;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use structopt::StructOpt;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex, RwLock,
    },
};

#[derive(StructOpt, Debug)]
pub struct NetworkOptions {
    /// Port to listen on (0 for random)
    #[structopt(short, long, default_value = "0")]
    pub port: u16,

    /// IP address to bind to
    #[structopt(short, long, default_value = "0.0.0.0", value_name = "ip")]
    pub bind: IpAddr,

    /// Disable local discovery
    #[structopt(short, long)]
    pub disable_local_discovery: bool,

    /// Explicit list of IP:PORT pairs of peers to connect to
    #[structopt(long)]
    pub peers: Vec<SocketAddr>,
}

impl NetworkOptions {
    pub fn listen_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }
}

impl Default for NetworkOptions {
    fn default() -> Self {
        Self {
            port: 0,
            bind: Ipv4Addr::UNSPECIFIED.into(),
            disable_local_discovery: false,
            peers: Vec::new(),
        }
    }
}

pub struct Network {
    inner: Arc<Inner>,
    local_addr: SocketAddr,
    port_forwarder: upnp::PortForwarder,
    _tasks: ScopedTaskSet,
}

impl Network {
    pub async fn new(this_replica_id: ReplicaId, options: &NetworkOptions) -> Result<Self> {
        let tasks = ScopedTaskSet::default();

        let listener = TcpListener::bind(options.listen_addr())
            .await
            .map_err(Error::Network)?;

        let local_addr = listener.local_addr().map_err(Error::Network)?;

        let port_forwarder = upnp::PortForwarder::new(local_addr.port());

        let (forget_tx, forget_rx) = mpsc::channel(1);

        let inner = Inner {
            this_replica_id,
            message_brokers: Mutex::new(HashMap::new()),
            indices: RwLock::new(IndexMap::default()),
            forget_tx,
            task_handle: tasks.handle().clone(),
            user_provided_peers: RwLock::new(HashSet::new()),
        };

        let inner = Arc::new(inner);

        tasks.spawn(inner.clone().run_listener(listener));

        if !options.disable_local_discovery {
            tasks.spawn(inner.clone().run_discovery(local_addr.port(), forget_rx));
        }

        for peer in &options.peers {
            tasks.spawn(inner.clone().add_user_provided_peer(*peer));
        }

        Ok(Self {
            inner,
            local_addr,
            port_forwarder,
            _tasks: tasks,
        })
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }

    pub fn handle(&self) -> Handle {
        Handle {
            inner: self.inner.clone(),
        }
    }
}

/// Handle for the network which can be cheaply cloned and sent to other threads.
#[derive(Clone)]
pub struct Handle {
    inner: Arc<Inner>,
}

impl Handle {
    /// Register a local repository into the network. This links the repository with all matching
    /// repositories of currently connected remote replicas as well as any replicas connected in
    /// the future. The repository is automatically deregistered when dropped.
    pub async fn register(&self, name: &str, repository: &Repository) -> bool {
        let index = repository.index();

        let id = if let Some(id) = self
            .inner
            .indices
            .write()
            .await
            .insert(name.to_owned(), index.clone())
        {
            id
        } else {
            return false;
        };

        for broker in self.inner.message_brokers.lock().await.values() {
            create_link(broker, id, name.to_owned(), index.clone()).await;
        }

        // Deregister the index when it gets closed.
        self.inner.task_handle.spawn({
            let closed = index.subscribe().closed();
            let inner = self.inner.clone();

            async move {
                closed.await;

                inner.indices.write().await.remove(id);

                for broker in inner.message_brokers.lock().await.values() {
                    broker.destroy_link(Local::new(id)).await;
                }
            }
        });

        true
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        &self.inner.this_replica_id
    }
}

#[derive(Debug)]
enum PeerSource {
    UserProvided(SocketAddr),
    Listener,
    LocalDiscovery(RuntimeId),
}

struct Inner {
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, MessageBroker>>,
    indices: RwLock<IndexMap>,
    forget_tx: Sender<RuntimeId>,
    task_handle: ScopedTaskHandle,
    user_provided_peers: RwLock<HashSet<SocketAddr>>,
}

impl Inner {
    async fn run_discovery(
        self: Arc<Self>,
        listener_port: u16,
        mut forget_rx: Receiver<RuntimeId>,
    ) {
        let (tx, mut rx) = mpsc::channel(1);
        let discovery = match ReplicaDiscovery::new(listener_port, tx) {
            Ok(discovery) => discovery,
            Err(error) => {
                log::error!("Failed to create ReplicaDiscovery: {}", error);
                return;
            }
        };

        let discover_task = async {
            while let Some((id, addr)) = rx.recv().await {
                self.task_handle
                    .spawn(self.clone().establish_discovered_connection(addr, id))
            }
        };

        let forget_task = async {
            while let Some(id) = forget_rx.recv().await {
                discovery.forget(&id).await;
            }
        };

        future::join(discover_task, forget_task).await;
    }

    async fn run_listener(self: Arc<Self>, listener: TcpListener) {
        loop {
            let (socket, addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("Failed to accept incoming TCP connection: {}", error);
                    break;
                }
            };

            log::debug!("New incoming TCP connection: {}", addr);

            self.task_handle.spawn(
                self.clone()
                    .handle_new_connection(socket, PeerSource::Listener),
            );
        }
    }

    async fn add_user_provided_peer(self: Arc<Self>, peer: SocketAddr) {
        if !self.user_provided_peers.write().await.insert(peer) {
            return;
        }

        self.establish_user_provided_connection(peer).await
    }

    #[async_recursion]
    async fn establish_user_provided_connection(self: Arc<Self>, addr: SocketAddr) {
        use std::cmp::min;

        let mut i: u32 = 0;

        let socket = loop {
            match TcpStream::connect(addr).await {
                Ok(socket) => {
                    break socket;
                }
                Err(error) => {
                    // TODO: Might be worth randomizing this somehow.
                    let sleep_duration = min(
                        Duration::from_secs(5),
                        Duration::from_millis(200 * 2u64.pow(min(i, 10))),
                    );
                    log::debug!(
                        "Failed to create outgoing TCP connection to {}: {}. Retrying in {:?}",
                        addr,
                        error,
                        sleep_duration
                    );
                    tokio::time::sleep(sleep_duration).await;
                    i += 1;
                }
            };
        };

        log::info!("New outgoing TCP connection: {} (User provided)", addr);
        self.handle_new_connection(socket, PeerSource::UserProvided(addr))
            .await
    }

    async fn establish_discovered_connection(
        self: Arc<Self>,
        addr: SocketAddr,
        discovery_id: RuntimeId,
    ) {
        let socket = match TcpStream::connect(addr).await {
            Ok(socket) => socket,
            Err(error) => {
                log::error!("Failed to create outgoing TCP connection: {}", error);
                return;
            }
        };

        log::info!("New outgoing TCP connection: {} (Locally discovered)", addr);
        self.handle_new_connection(socket, PeerSource::LocalDiscovery(discovery_id))
            .await
    }

    async fn handle_new_connection(self: Arc<Self>, socket: TcpStream, peer_source: PeerSource) {
        let mut stream = TcpObjectStream::new(socket);
        let their_replica_id = match perform_handshake(&mut stream, &self.this_replica_id).await {
            Ok(replica_id) => replica_id,
            Err(error) => {
                log::error!("Failed to perform handshake: {}", error);
                return;
            }
        };

        let mut brokers = self.message_brokers.lock().await;

        match brokers.entry(their_replica_id) {
            Entry::Occupied(entry) => entry.get().add_connection(stream).await,
            Entry::Vacant(entry) => {
                log::info!("Connected to replica {:?}", their_replica_id);

                let broker = MessageBroker::new(
                    their_replica_id,
                    stream,
                    Box::pin(self.clone().on_finish(their_replica_id, peer_source)),
                )
                .await;

                for (id, holder) in &self.indices.read().await.map {
                    create_link(&broker, *id, holder.name.clone(), holder.index.clone()).await;
                }

                entry.insert(broker);
            }
        }
    }

    async fn on_finish(self: Arc<Self>, replica_id: ReplicaId, peer_source: PeerSource) {
        log::info!("Disconnected from replica {:?}", replica_id);

        self.message_brokers.lock().await.remove(&replica_id);

        match peer_source {
            PeerSource::LocalDiscovery(discovery_id) => {
                // LocalDiscovery keeps track of what peers it passed to us to avoid passing it
                // multiple times. We now tell it to lose track of this particular peer so that
                // next time it appears locally it'll let us know about it again.
                self.forget_tx.send(discovery_id).await.unwrap_or(())
            }
            PeerSource::UserProvided(addr) => {
                // We attempt to re-establish connections to user provided peers right a way.
                //
                // NOTE: For some reason if we don't spawn here (i.e. call the self.establish_...
                // function directly), then the function halts on TcpStream::connect ¯\_(ツ)_/¯.
                tokio::task::spawn(
                    async move { self.establish_user_provided_connection(addr).await },
                );
            }
            PeerSource::Listener => {}
        }
    }
}

#[derive(Default)]
struct IndexMap {
    map: HashMap<RepositoryId, IndexHolder>,
    next_id: RepositoryId,
}

impl IndexMap {
    fn insert(&mut self, name: String, index: Index) -> Option<RepositoryId> {
        let id = self.next_id;

        match self.map.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(IndexHolder { index, name });
                self.next_id = self.next_id.wrapping_add(1);
                Some(id)
            }
            Entry::Occupied(_) => None,
        }
    }

    fn remove(&mut self, id: RepositoryId) {
        self.map.remove(&id);
    }
}

struct IndexHolder {
    index: Index,
    name: String,
}

async fn perform_handshake(
    stream: &mut TcpObjectStream,
    this_replica_id: &ReplicaId,
) -> io::Result<ReplicaId> {
    stream.write(this_replica_id).await?;
    stream.read().await
}

async fn create_link(broker: &MessageBroker, id: RepositoryId, name: String, index: Index) {
    // TODO: creating implicit link if the local and remote repository names are the same.
    // Eventually the links will be explicit.

    let local_id = Local::new(id);
    let local_name = Local::new(name.clone());
    let remote_name = Remote::new(name);

    broker
        .create_link(index, local_id, local_name, remote_name)
        .await
}
