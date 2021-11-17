mod client;
mod connection;
mod dht_discovery;
mod ip_stack;
mod local_discovery;
mod message;
mod message_broker;
mod object_stream;
mod server;
#[cfg(test)]
mod tests;

use self::{
    connection::{ConnectionDeduplicator, ConnectionDirection, ConnectionPermit},
    dht_discovery::DhtDiscovery,
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    object_stream::TcpObjectStream,
};
use crate::{
    crypto::Hashable,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
    repository::{Repository, RepositoryId},
    scoped_task::{self, ScopedJoinHandle, ScopedTaskSet},
    upnp,
};
use btdht::{InfoHash, INFO_HASH_LEN};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    future::Future,
    io, iter,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Weak},
    time::Duration,
};
use structopt::StructOpt;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex, RwLock},
    task, time,
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

    /// Disable UPnP
    #[structopt(long)]
    pub disable_upnp: bool,

    /// Disable DHT
    #[structopt(long)]
    pub disable_dht: bool,

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
            disable_upnp: false,
            disable_dht: false,
            peers: Vec::new(),
        }
    }
}

pub struct Network {
    inner: Arc<Inner>,
    // We keep tasks here instead of in Inner because we want them to be
    // destroyed when Network is Dropped.
    _tasks: Arc<RwLock<Tasks>>,
    _port_forwarder: Option<upnp::PortForwarder>,
}

impl Network {
    pub async fn new(this_replica_id: ReplicaId, options: &NetworkOptions) -> Result<Self> {
        let listener = TcpListener::bind(options.listen_addr())
            .await
            .map_err(Error::Network)?;

        let local_addr = listener.local_addr().map_err(Error::Network)?;

        let dht_sockets = if !options.disable_dht {
            Some(dht_discovery::bind().await.map_err(Error::Network)?)
        } else {
            None
        };

        let port_forwarder = if !options.disable_upnp {
            let dht_port_v4 = dht_sockets
                .as_ref()
                .and_then(|sockets| sockets.v4())
                .map(|socket| socket.local_addr())
                .transpose()
                .map_err(Error::Network)?
                .map(|addr| addr.port());

            // TODO: the ipv6 port typically doesn't need to be port-mappted but it might need to
            // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.

            Some(upnp::PortForwarder::new(
                iter::once(upnp::Mapping {
                    external: local_addr.port(),
                    internal: local_addr.port(),
                    protocol: upnp::Protocol::Tcp,
                })
                .chain(dht_port_v4.map(|port| upnp::Mapping {
                    external: port,
                    internal: port,
                    protocol: upnp::Protocol::Udp,
                })),
            ))
        } else {
            None
        };

        let dht_discovery = if let Some(dht_sockets) = dht_sockets {
            Some(DhtDiscovery::new(dht_sockets, local_addr.port()).await)
        } else {
            None
        };

        let tasks = Arc::new(RwLock::new(Tasks::default()));

        let (dht_peer_found_tx, mut dht_peer_found_rx) = mpsc::unbounded_channel();

        let inner = Arc::new(Inner {
            local_addr,
            this_replica_id,
            message_brokers: Mutex::new(HashMap::new()),
            indices: RwLock::new(HashMap::default()),
            dht_discovery,
            dht_lookups: Default::default(),
            dht_peer_found_tx,
            connection_deduplicator: ConnectionDeduplicator::new(),
            tasks: Arc::downgrade(&tasks),
        });

        let network = Self {
            inner: inner.clone(),
            _tasks: tasks,
            _port_forwarder: port_forwarder,
        };

        // Gets destroyed once dht_peer_found_tx is destroyed
        task::spawn({
            let weak = Arc::downgrade(&inner);
            async move {
                while let Some(peer_addr) = dht_peer_found_rx.recv().await {
                    let inner = match weak.upgrade() {
                        Some(inner) => inner,
                        None => return,
                    };

                    inner
                        .spawn(inner.clone().establish_dht_connection(peer_addr))
                        .await;
                }
            }
        });

        inner.spawn(inner.clone().run_listener(listener)).await;

        inner
            .enable_local_discovery(!options.disable_local_discovery)
            .await;

        for peer in &options.peers {
            inner
                .clone()
                .establish_user_provided_connection(*peer)
                .await;
        }

        Ok(network)
    }

    pub fn local_addr(&self) -> &SocketAddr {
        &self.inner.local_addr
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
    pub async fn register(&self, repository: &Repository) -> bool {
        let id = match repository.get_id().await {
            Ok(Some(id)) => id,
            Ok(None) => {
                log::warn!("not registering repository - missing id");
                return false;
            }
            Err(error) => {
                log::error!(
                    "not registering repository - failed to retrieve id: {}",
                    error
                );
                return false;
            }
        };

        let info_hash = repository_info_hash(&id);

        match self.inner.indices.write().await.entry(info_hash) {
            Entry::Occupied(_) => {
                log::warn!("not registering repository - already registered");
                return false;
            }
            Entry::Vacant(entry) => {
                entry.insert(IndexHolder {
                    index: repository.index().clone(),
                });
            }
        }

        for broker in self.inner.message_brokers.lock().await.values() {
            broker
                .create_link(info_hash, repository.index().clone())
                .await
        }

        // Deregister the index when it gets closed.
        self.inner
            .spawn({
                let closed = repository.index().subscribe().closed();
                let inner = self.inner.clone();

                async move {
                    closed.await;

                    inner.indices.write().await.remove(&info_hash);

                    for broker in inner.message_brokers.lock().await.values() {
                        broker.destroy_link(info_hash).await;
                    }

                    inner.disable_dht_for_repository(&info_hash).await;
                }
            })
            .await;

        self.enable_dht_for_repository(repository).await;

        true
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        &self.inner.this_replica_id
    }

    pub async fn enable_dht_for_repository(&self, repository: &Repository) {
        let id = if let Ok(Some(id)) = repository.get_id().await {
            repository_info_hash(&id)
        } else {
            log::warn!("can't enable DHT for repository - missing id");
            return;
        };

        self.inner.enable_dht_for_repository(id).await
    }

    pub async fn disable_dht_for_repository(&self, repository: &Repository) {
        let id = if let Ok(Some(id)) = repository.get_id().await {
            repository_info_hash(&id)
        } else {
            log::warn!("can't disable DHT for repository - missing id");
            return;
        };

        self.inner.disable_dht_for_repository(&id).await
    }

    pub async fn is_dht_for_repository_enabled(&self, repository: &Repository) -> bool {
        let id = if let Ok(Some(id)) = repository.get_id().await {
            repository_info_hash(&id)
        } else {
            return false;
        };

        self.inner.dht_lookups.lock().await.get(&id).is_some()
    }
}

#[derive(Default)]
struct Tasks {
    local_discovery: Option<ScopedJoinHandle<()>>,
    other: ScopedTaskSet,
}

struct Inner {
    local_addr: SocketAddr,
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, MessageBroker>>,
    indices: RwLock<HashMap<InfoHash, IndexHolder>>,
    dht_discovery: Option<DhtDiscovery>,
    dht_lookups: Mutex<HashMap<InfoHash, dht_discovery::LookupRequest>>,
    dht_peer_found_tx: mpsc::UnboundedSender<SocketAddr>,
    connection_deduplicator: ConnectionDeduplicator,
    // Note that unwrapping the upgraded weak pointer should be fine because if the underlying Arc
    // was Dropped, we would not be asking for the upgrade in the first place.
    tasks: Weak<RwLock<Tasks>>,
}

impl Inner {
    async fn enable_local_discovery(self: &Arc<Self>, enable: bool) {
        let tasks_arc = self.tasks.upgrade().unwrap();
        let mut tasks = tasks_arc.write().await;

        if !enable {
            tasks.local_discovery = None;
            return;
        }

        if tasks.local_discovery.is_some() {
            return;
        }

        let self_ = self.clone();
        tasks.local_discovery = Some(scoped_task::spawn(async move {
            let port = self_.local_addr.port();
            self_.run_local_discovery(port).await;
        }));
    }

    async fn run_local_discovery(self: Arc<Self>, listener_port: u16) {
        let discovery = match LocalDiscovery::new(listener_port) {
            Ok(discovery) => discovery,
            Err(error) => {
                log::error!("Failed to create LocalDiscovery: {}", error);
                return;
            }
        };

        while let Some(addr) = discovery.recv().await {
            let tasks_arc = self.tasks.upgrade().unwrap();
            let tasks = tasks_arc.write().await;

            tasks
                .other
                .spawn(self.clone().establish_discovered_connection(addr))
        }
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

            if let Some(permit) = self
                .connection_deduplicator
                .reserve(addr, ConnectionDirection::Incoming)
            {
                self.spawn(
                    self.clone()
                        .handle_new_connection(socket, PeerSource::Listener, permit),
                )
                .await
            }
        }
    }

    // Periodically search for peers for the given repository and announce it on the DHT.
    async fn enable_dht_for_repository(&self, id: InfoHash) {
        if let Some(dht_discovery) = &self.dht_discovery {
            let mut dht_lookups = self.dht_lookups.lock().await;

            match dht_lookups.entry(id) {
                Entry::Occupied(_) => {}
                Entry::Vacant(entry) => {
                    entry.insert(dht_discovery.lookup(id, self.dht_peer_found_tx.clone()));
                }
            };
        }
    }

    async fn disable_dht_for_repository(&self, id: &InfoHash) {
        self.dht_lookups.lock().await.remove(id);
    }

    async fn establish_user_provided_connection(self: Arc<Self>, addr: SocketAddr) {
        self.spawn({
            let inner = self.clone();
            async move {
                loop {
                    let permit = if let Some(permit) = inner
                        .connection_deduplicator
                        .reserve(addr, ConnectionDirection::Outgoing)
                    {
                        permit
                    } else {
                        return;
                    };

                    let socket = inner.keep_connecting(addr).await;

                    inner
                        .clone()
                        .handle_new_connection(socket, PeerSource::UserProvided, permit)
                        .await;
                }
            }
        })
        .await
    }

    async fn establish_discovered_connection(self: Arc<Self>, addr: SocketAddr) {
        let permit = if let Some(permit) = self
            .connection_deduplicator
            .reserve(addr, ConnectionDirection::Outgoing)
        {
            permit
        } else {
            return;
        };

        let socket = match TcpStream::connect(addr).await {
            Ok(socket) => socket,
            Err(error) => {
                log::error!("Failed to create outgoing TCP connection: {}", error);
                return;
            }
        };

        self.handle_new_connection(socket, PeerSource::LocalDiscovery, permit)
            .await;
    }

    async fn establish_dht_connection(self: Arc<Self>, addr: SocketAddr) {
        let permit = if let Some(permit) = self
            .connection_deduplicator
            .reserve(addr, ConnectionDirection::Outgoing)
        {
            permit
        } else {
            return;
        };

        // TODO: we should give up after a timeout
        let socket = self.keep_connecting(addr).await;

        self.handle_new_connection(socket, PeerSource::Dht, permit)
            .await;
    }

    async fn keep_connecting(&self, addr: SocketAddr) -> TcpStream {
        let mut i = 0;

        loop {
            match TcpStream::connect(addr).await {
                Ok(socket) => {
                    return socket;
                }
                Err(error) => {
                    // TODO: Might be worth randomizing this somehow.
                    let sleep_duration = Duration::from_secs(5)
                        .min(Duration::from_millis(200 * 2u64.pow(i.min(10))));
                    log::debug!(
                        "Failed to create outgoing TCP connection to {}: {}. Retrying in {:?}",
                        addr,
                        error,
                        sleep_duration
                    );
                    time::sleep(sleep_duration).await;
                    i = i.saturating_add(1);
                }
            }
        }
    }

    async fn handle_new_connection(
        self: Arc<Self>,
        socket: TcpStream,
        peer_source: PeerSource,
        permit: ConnectionPermit,
    ) {
        let addr = permit.addr();

        log::info!("New {} TCP connection: {}", peer_source, addr);

        let mut stream = TcpObjectStream::new(socket);
        let their_replica_id = match perform_handshake(&mut stream, self.this_replica_id).await {
            Ok(replica_id) => replica_id,
            Err(error) => {
                log::error!("Failed to perform handshake: {}", error);
                return;
            }
        };

        // prevent self-connections.
        if their_replica_id == self.this_replica_id {
            log::debug!("Connection from self, discarding");
            return;
        }

        let released = permit.released();

        let mut brokers = self.message_brokers.lock().await;

        match brokers.entry(their_replica_id) {
            Entry::Occupied(entry) => entry.get().add_connection(stream, permit).await,
            Entry::Vacant(entry) => {
                log::info!("Connected to replica {:?}", their_replica_id);

                let broker = MessageBroker::new(their_replica_id, stream, permit).await;

                // TODO: for DHT connection we should only link the repository for which we did the
                // lookup but make sure we correctly handle edge cases, for example, when we have
                // more than one repository shared with the peer.
                for (id, holder) in &*self.indices.read().await {
                    broker.create_link(*id, holder.index.clone()).await;
                }

                entry.insert(broker);
            }
        }

        drop(brokers);

        released.notified().await;
        log::info!("Lost {} TCP connection: {}", peer_source, addr);

        // Remove the broker if it has no more connections.
        let mut brokers = self.message_brokers.lock().await;
        if let Entry::Occupied(entry) = brokers.entry(their_replica_id) {
            if !entry.get().has_connections() {
                entry.remove();
            }
        }
    }

    async fn spawn<Fut>(&self, f: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let tasks_arc = self.tasks.upgrade().unwrap();
        let tasks = tasks_arc.write().await;
        tasks.other.spawn(f);
    }
}

struct IndexHolder {
    index: Index,
}

async fn perform_handshake(
    stream: &mut TcpObjectStream,
    this_replica_id: ReplicaId,
) -> io::Result<ReplicaId> {
    stream.write(&this_replica_id).await?;
    stream.read().await
}

// Calculate info hash for a repository id.
fn repository_info_hash(id: &RepositoryId) -> InfoHash {
    // Calculate the info hash by hashing the id with SHA3-256 and taking the first 20 bytes.
    // (bittorrent uses SHA-1 but that is less secure).
    // `unwrap` is OK because the byte slice has the correct length.
    InfoHash::try_from(&id.as_ref().hash().as_ref()[..INFO_HASH_LEN]).unwrap()
}

#[derive(Clone, Copy, Debug)]
pub enum PeerSource {
    UserProvided,
    Listener,
    LocalDiscovery,
    Dht,
}

impl fmt::Display for PeerSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PeerSource::Listener => write!(f, "incoming"),
            PeerSource::UserProvided => write!(f, "outgoing (user provided)"),
            PeerSource::LocalDiscovery => write!(f, "outgoing (locally discovered)"),
            PeerSource::Dht => write!(f, "outgoing (found via DHT)"),
        }
    }
}
