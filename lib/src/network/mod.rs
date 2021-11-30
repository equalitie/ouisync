mod client;
mod connection;
mod crypto;
mod dht_discovery;
mod ip_stack;
mod local_discovery;
mod message;
mod message_broker;
mod message_dispatcher;
mod message_io;
mod runtime_id;
mod server;
#[cfg(test)]
mod tests;

use self::{
    connection::{ConnectionDeduplicator, ConnectionDirection, ConnectionPermit},
    dht_discovery::DhtDiscovery,
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    runtime_id::RuntimeId,
};
use crate::{
    error::{Error, Result},
    index::Index,
    repository::{PublicRepositoryId, Repository, SecretRepositoryId},
    scoped_task::{self, ScopedJoinHandle, ScopedTaskSet},
    upnp,
};
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
    io::{AsyncReadExt, AsyncWriteExt},
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
    pub async fn new(options: &NetworkOptions) -> Result<Self> {
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

            // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
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
            this_runtime_id: rand::random(),
            message_brokers: Mutex::new(HashMap::new()),
            indices: RwLock::new(HashMap::default()),
            dht_discovery,
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
        let sid = match repository.get_id().await {
            Ok(id) => id,
            Err(error) => {
                log::warn!("not registering repository - missing id ({})", error);
                return false;
            }
        };

        let pid = sid.public();

        match self.inner.indices.write().await.entry(pid) {
            Entry::Occupied(_) => {
                log::warn!("not registering repository - already registered");
                return false;
            }
            Entry::Vacant(entry) => {
                entry.insert(IndexHolder {
                    index: repository.index().clone(),
                    id: sid,
                    dht_lookup: self.inner.start_dht_lookup(pid),
                });
            }
        }

        for broker in self.inner.message_brokers.lock().await.values() {
            broker.create_link(sid, repository.index().clone()).await
        }

        // Deregister the index when it gets closed.
        self.inner
            .spawn({
                let closed = repository.index().subscribe().closed();
                let inner = self.inner.clone();

                async move {
                    closed.await;

                    inner.indices.write().await.remove(&pid);

                    for broker in inner.message_brokers.lock().await.values() {
                        broker.destroy_link(pid).await;
                    }
                }
            })
            .await;

        true
    }

    pub async fn enable_dht_for_repository(&self, repository: &Repository) {
        let id = if let Ok(id) = repository.get_id().await {
            id.public()
        } else {
            log::warn!("not enabling DHT for repository - missing id");
            return;
        };

        let mut indices = self.inner.indices.write().await;
        let holder = if let Some(holder) = indices.get_mut(&id) {
            holder
        } else {
            log::warn!("not enabling DHT for repository - not registered");
            return;
        };

        if holder.dht_lookup.is_none() {
            holder.dht_lookup = self.inner.start_dht_lookup(id);
        }
    }

    pub async fn disable_dht_for_repository(&self, repository: &Repository) {
        let id = if let Ok(id) = repository.get_id().await {
            id.public()
        } else {
            log::warn!("not disabling DHT for repository - missing id");
            return;
        };

        if let Some(holder) = self.inner.indices.write().await.get_mut(&id) {
            holder.dht_lookup = None;
        } else {
            log::warn!("not disabling DHT for repository - not registered");
        }
    }

    pub async fn is_dht_for_repository_enabled(&self, repository: &Repository) -> bool {
        let id = if let Ok(id) = repository.get_id().await {
            id.public()
        } else {
            return false;
        };

        if let Some(holder) = self.inner.indices.read().await.get(&id) {
            holder.dht_lookup.is_some()
        } else {
            false
        }
    }
}

#[derive(Default)]
struct Tasks {
    local_discovery: Option<ScopedJoinHandle<()>>,
    other: ScopedTaskSet,
}

struct Inner {
    local_addr: SocketAddr,
    this_runtime_id: RuntimeId,
    message_brokers: Mutex<HashMap<RuntimeId, MessageBroker>>,
    indices: RwLock<HashMap<PublicRepositoryId, IndexHolder>>,
    dht_discovery: Option<DhtDiscovery>,
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
        let discovery = match LocalDiscovery::new(self.this_runtime_id, listener_port) {
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

    fn start_dht_lookup(&self, id: PublicRepositoryId) -> Option<dht_discovery::LookupRequest> {
        self.dht_discovery
            .as_ref()
            .map(|dht| dht.lookup(id.to_info_hash(), self.dht_peer_found_tx.clone()))
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
        mut stream: TcpStream,
        peer_source: PeerSource,
        permit: ConnectionPermit,
    ) {
        let addr = permit.addr();

        log::info!("New {} TCP connection: {}", peer_source, addr);

        let that_runtime_id = match perform_handshake(&mut stream, self.this_runtime_id).await {
            Ok(replica_id) => replica_id,
            Err(error) => {
                log::error!("Failed to perform handshake: {}", error);
                return;
            }
        };

        // prevent self-connections.
        if that_runtime_id == self.this_runtime_id {
            log::debug!("Connection from self, discarding");
            return;
        }

        let released = permit.released();

        let mut brokers = self.message_brokers.lock().await;

        match brokers.entry(that_runtime_id) {
            Entry::Occupied(entry) => entry.get().add_connection(stream, permit).await,
            Entry::Vacant(entry) => {
                log::info!("Connected to replica {:?}", that_runtime_id);

                let broker =
                    MessageBroker::new(self.this_runtime_id, that_runtime_id, stream, permit);

                // TODO: for DHT connection we should only link the repository for which we did the
                // lookup but make sure we correctly handle edge cases, for example, when we have
                // more than one repository shared with the peer.
                for holder in self.indices.read().await.values() {
                    broker.create_link(holder.id, holder.index.clone()).await;
                }

                entry.insert(broker);
            }
        };

        drop(brokers);

        released.notified().await;
        log::info!("Lost {} TCP connection: {}", peer_source, addr);

        // Remove the broker if it has no more connections.
        let mut brokers = self.message_brokers.lock().await;
        if let Entry::Occupied(entry) = brokers.entry(that_runtime_id) {
            if !entry.get().has_connections().await {
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
    id: SecretRepositoryId,
    dht_lookup: Option<dht_discovery::LookupRequest>,
}

// Exchange runtime ids with the peer. Returns their runtime id.
async fn perform_handshake(
    stream: &mut TcpStream,
    this_runtime_id: RuntimeId,
) -> io::Result<RuntimeId> {
    stream.write_all(this_runtime_id.as_ref()).await?;

    let mut their_runtime_id = [0; RuntimeId::SIZE];
    stream.read_exact(&mut their_runtime_id).await?;

    Ok(their_runtime_id.into())
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
