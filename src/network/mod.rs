mod client;
mod connection;
mod message;
mod message_broker;
mod object_stream;
mod replica_discovery;
mod server;
#[cfg(test)]
mod tests;

use self::{
    connection::{ConnectionDeduplicator, ConnectionDirection, ConnectionPermit},
    message::RepositoryId,
    message_broker::MessageBroker,
    object_stream::TcpObjectStream,
    replica_discovery::{ReplicaDiscovery, RuntimeId},
};
use crate::{
    crypto::Hashable,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
    repository::Repository,
    scoped_task::{ScopedTaskHandle, ScopedTaskSet},
    tagged::{Local, Remote},
    upnp,
};
use async_recursion::async_recursion;
use btdht::{DhtEvent, InfoHash, MainlineDht, INFO_HASH_LEN};
use futures_util::future;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    convert::TryFrom,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use structopt::StructOpt;
use tokio::{
    net::{self, TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, Mutex, RwLock},
    time,
};

// Hardcoded DHT routers to bootstrap the DHT against.
const DHT_ROUTERS: &[&str] = &["router.bittorrent.com:6881", "dht.transmissionbt.com:6881"];

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
    local_addr: SocketAddr,
    _port_forwarder: Option<upnp::PortForwarder>,
    _tasks: ScopedTaskSet,
}

impl Network {
    pub async fn new(this_replica_id: ReplicaId, options: &NetworkOptions) -> Result<Self> {
        let tasks = ScopedTaskSet::default();

        let listener = TcpListener::bind(options.listen_addr())
            .await
            .map_err(Error::Network)?;

        let local_addr = listener.local_addr().map_err(Error::Network)?;

        let port_forwarder = if !options.disable_upnp {
            Some(upnp::PortForwarder::new(local_addr.port()))
        } else {
            None
        };

        let (dht, dht_event_rx) = if !options.disable_dht {
            // TODO: consider port-forward this socket as well.
            let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
                .await
                .map_err(Error::Network)?;

            // TODO: load the DHT state from a previous save if it exists.
            let (dht, event_rx) = MainlineDht::builder()
                .add_routers(dht_router_addresses().await)
                .set_read_only(false)
                .set_announce_port(local_addr.port())
                .start(socket);

            (Some(dht), Some(event_rx))
        } else {
            (None, None)
        };

        let inner = Inner {
            this_replica_id,
            message_brokers: Mutex::new(HashMap::new()),
            indices: RwLock::new(IndexMap::default()),
            task_handle: tasks.handle().clone(),
            dht,
            connection_deduplicator: ConnectionDeduplicator::new(),
        };

        let inner = Arc::new(inner);

        tasks.spawn(inner.clone().run_listener(listener));

        if !options.disable_local_discovery {
            tasks.spawn(inner.clone().run_local_discovery(local_addr.port()));
        }

        for peer in &options.peers {
            tasks.spawn(inner.clone().establish_user_provided_connection(*peer));
        }

        if let Some(event_rx) = dht_event_rx {
            tasks.spawn(inner.clone().run_dht(event_rx));
        }

        Ok(Self {
            inner,
            local_addr,
            _port_forwarder: port_forwarder,
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

        self.inner.find_peers_for_repository(name);

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
    Dht,
}

struct Inner {
    this_replica_id: ReplicaId,
    message_brokers: Mutex<HashMap<ReplicaId, MessageBroker>>,
    indices: RwLock<IndexMap>,
    task_handle: ScopedTaskHandle,
    dht: Option<MainlineDht>,
    connection_deduplicator: ConnectionDeduplicator,
}

impl Inner {
    async fn run_local_discovery(self: Arc<Self>, listener_port: u16) {
        let (tx, mut rx) = mpsc::channel(1);
        let _discovery = match ReplicaDiscovery::new(listener_port, tx) {
            Ok(discovery) => discovery,
            Err(error) => {
                log::error!("Failed to create ReplicaDiscovery: {}", error);
                return;
            }
        };

        while let Some((id, addr)) = rx.recv().await {
            self.task_handle
                .spawn(self.clone().establish_discovered_connection(addr, id))
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
                log::debug!("New incoming TCP connection: {}", addr);

                self.task_handle.spawn(self.clone().handle_new_connection(
                    socket,
                    PeerSource::Listener,
                    permit,
                ));
            }
        }
    }

    async fn run_dht(self: Arc<Self>, mut event_rx: mpsc::UnboundedReceiver<DhtEvent>) {
        // To deduplicate found peers.
        let mut lookups: HashMap<_, HashSet<_>> = HashMap::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                DhtEvent::BootstrapCompleted => log::info!("DHT bootstrap complete"),
                DhtEvent::BootstrapFailed => {
                    log::error!("DHT bootstrap failed");
                    break;
                }
                DhtEvent::PeerFound(info_hash, addr) => {
                    log::debug!("DHT found peer for {:?}: {}", info_hash, addr);
                    lookups.entry(info_hash).or_default().insert(addr);
                }
                DhtEvent::LookupCompleted(info_hash) => {
                    log::debug!("DHT lookup for {:?} complete", info_hash);

                    for addr in lookups.remove(&info_hash).unwrap_or_default() {
                        self.task_handle
                            .spawn(self.clone().establish_dht_connection(addr));
                    }
                }
            }
        }
    }

    // Start DHT lookup for the peers that have the specified repository.
    // TODO: use some unique id instead of name.
    fn find_peers_for_repository(&self, name: &str) {
        let dht = if let Some(dht) = &self.dht {
            dht
        } else {
            return;
        };

        // Calculate the info hash by hashing the name with SHA-256 and taking the first 20 bytes.
        // (bittorrent uses SHA-1 but that is less secure).
        // `unwrap` is OK because the byte slice has the correct length.
        let info_hash =
            InfoHash::try_from(&name.as_bytes().hash().as_ref()[..INFO_HASH_LEN]).unwrap();

        // find peers for the repo and also announce that we have it.
        dht.search(info_hash, true);

        // TODO: periodically re-announce the info-hash.
    }

    #[async_recursion]
    async fn establish_user_provided_connection(self: Arc<Self>, addr: SocketAddr) {
        // TODO: keep re-establishing the connection after it gets closed.

        let permit = if let Some(permit) = self
            .connection_deduplicator
            .reserve(addr, ConnectionDirection::Outgoing)
        {
            permit
        } else {
            return;
        };

        let socket = self.keep_connecting(addr).await;

        log::info!("New outgoing TCP connection: {} (User provided)", addr);
        self.handle_new_connection(socket, PeerSource::UserProvided(addr), permit)
            .await
    }

    async fn establish_discovered_connection(
        self: Arc<Self>,
        addr: SocketAddr,
        discovery_id: RuntimeId,
    ) {
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

        log::info!("New outgoing TCP connection: {} (Locally discovered)", addr);
        self.handle_new_connection(socket, PeerSource::LocalDiscovery(discovery_id), permit)
            .await
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

        log::info!("New outgoing TCP connection: {} (Found via DHT)", addr);
        self.handle_new_connection(socket, PeerSource::Dht, permit)
            .await
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
        _peer_source: PeerSource,
        permit: ConnectionPermit,
    ) {
        let mut stream = TcpObjectStream::new(socket);
        let their_replica_id = match perform_handshake(&mut stream, &self.this_replica_id).await {
            Ok(replica_id) => replica_id,
            Err(error) => {
                log::error!("Failed to perform handshake: {}", error);
                return;
            }
        };

        // TODO: prevent self-connections.

        let mut brokers = self.message_brokers.lock().await;

        match brokers.entry(their_replica_id) {
            Entry::Occupied(entry) => entry.get().add_connection(stream, permit).await,
            Entry::Vacant(entry) => {
                log::info!("Connected to replica {:?}", their_replica_id);

                let broker = MessageBroker::new(their_replica_id, stream, permit).await;

                for (id, holder) in &self.indices.read().await.map {
                    create_link(&broker, *id, holder.name.clone(), holder.index.clone()).await;
                }

                entry.insert(broker);
            }
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

async fn dht_router_addresses() -> Vec<SocketAddr> {
    future::join_all(DHT_ROUTERS.iter().map(net::lookup_host))
        .await
        .into_iter()
        .filter_map(|result| result.ok())
        .flatten()
        .collect()
}
