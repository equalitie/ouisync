mod client;
mod connection;
mod crypto;
pub mod dht_discovery;
mod ip_stack;
mod local_discovery;
mod message;
mod message_broker;
mod message_dispatcher;
mod message_io;
mod protocol;
mod request_limiter;
mod server;
mod socket;
#[cfg(test)]
mod tests;
mod upnp;

use self::{
    connection::{ConnectionDeduplicator, ConnectionDirection, ConnectionPermit},
    dht_discovery::DhtDiscovery,
    ip_stack::IpStack,
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    protocol::{RuntimeId, Version, VERSION},
};
use crate::{
    config::{ConfigKey, ConfigStore},
    error::Error,
    index::Index,
    repository::RepositoryId,
    scoped_task::{self, ScopedJoinHandle, ScopedTaskSet},
};
use btdht::{InfoHash, INFO_HASH_LEN};
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    fmt,
    future::Future,
    io, iter,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex as BlockingMutex, Weak},
    time::Duration,
};
use structopt::StructOpt;
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch, Mutex},
    task, time,
};

const LAST_USED_TCP_PORT_KEY: ConfigKey<u16> = ConfigKey::new(
    "last_used_tcp_port",
    "The value stored in this file is the last used TCP port for listening on incoming\n\
     connections. It is used to avoid binding to a random port every time the application starts.\n\
     This, in turn, is mainly useful for users who can't or don't want to use UPnP and have to\n\
     default to manually setting up port forwarding on their routers.\n\
     \n\
     The value is not used when the user specifies the --port option on the command line.\n\
     However, it may still be overwritten.",
);

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
    _tasks: Arc<Tasks>,
    _port_forwarder: Option<upnp::PortForwarder>,
}

impl Network {
    pub async fn new(options: &NetworkOptions, config: ConfigStore) -> Result<Self, NetworkError> {
        let listener = Self::bind_listener(options.listen_addr(), &config).await?;
        let listener_local_addr = listener.local_addr()?;

        let dht_sockets = if !options.disable_dht {
            Some(dht_discovery::bind(&config).await?)
        } else {
            None
        };

        let dht_local_addrs = dht_sockets
            .as_ref()
            .map(|sockets| sockets.as_ref().try_map(|socket| socket.local_addr()))
            .transpose()?;

        let port_forwarder = if !options.disable_upnp {
            let dht_port_v4 = dht_sockets
                .as_ref()
                .and_then(|sockets| sockets.v4())
                .map(|socket| socket.local_addr())
                .transpose()?
                .map(|addr| addr.port());

            // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
            // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.

            Some(upnp::PortForwarder::new(
                iter::once(upnp::Mapping {
                    external: listener_local_addr.port(),
                    internal: listener_local_addr.port(),
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
            Some(DhtDiscovery::new(dht_sockets, listener_local_addr.port()).await)
        } else {
            None
        };

        let tasks = Arc::new(Tasks::default());

        let (dht_peer_found_tx, mut dht_peer_found_rx) = mpsc::unbounded_channel();

        let (on_protocol_mismatch_tx, on_protocol_mismatch_rx) = watch::channel(None);

        let inner = Arc::new(Inner {
            listener_local_addr,
            this_runtime_id: rand::random(),
            state: Mutex::new(State {
                message_brokers: HashMap::new(),
                registry: Slab::new(),
            }),
            dht_local_addrs,
            dht_discovery,
            dht_peer_found_tx,
            connection_deduplicator: ConnectionDeduplicator::new(),
            on_protocol_mismatch_tx,
            on_protocol_mismatch_rx,
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
                    if let Some(inner) = weak.upgrade() {
                        inner.spawn(inner.clone().establish_dht_connection(peer_addr));
                    }
                }
            }
        });

        inner.spawn(inner.clone().run_listener(listener));

        inner
            .enable_local_discovery(!options.disable_local_discovery)
            .await;

        for peer in &options.peers {
            inner.clone().establish_user_provided_connection(*peer);
        }

        Ok(network)
    }

    pub fn listener_local_addr(&self) -> &SocketAddr {
        &self.inner.listener_local_addr
    }

    pub fn dht_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.inner.dht_local_addrs.as_ref()?.v4()
    }

    pub fn dht_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.inner.dht_local_addrs.as_ref()?.v6()
    }

    pub fn handle(&self) -> Handle {
        Handle {
            inner: self.inner.clone(),
        }
    }

    pub fn collect_peer_info(&self) -> HashMap<SocketAddr, HashSet<ConnectionDirection>> {
        self.inner.connection_deduplicator.collect_peer_info()
    }

    // If the user did not specify (through NetworkOptions) the preferred port, then try to use
    // the one used last time. If that fails, or if this is the first time the app is running,
    // then use a random port.
    async fn bind_listener(
        preferred_addr: SocketAddr,
        config: &ConfigStore,
    ) -> Result<TcpListener, NetworkError> {
        Ok(socket::bind(preferred_addr, config.entry(LAST_USED_TCP_PORT_KEY)).await?)
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
    /// the future. The repository is automatically deregistered when the returned handle is
    /// dropped.
    pub async fn register(&self, index: Index) -> Registration {
        // TODO: consider disabling DHT by default, for privacy reasons.
        let dht = self
            .inner
            .start_dht_lookup(repository_info_hash(index.repository_id()));

        let mut network_state = self.inner.state.lock().await;

        let key = network_state.registry.insert(RegistrationHolder {
            index: index.clone(),
            dht,
        });

        network_state.create_link(index);

        Registration {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Subscribe to network notification events.
    pub fn on_protocol_mismatch(&self) -> watch::Receiver<Option<()>> {
        self.inner.on_protocol_mismatch_rx.clone()
    }
}

pub struct Registration {
    inner: Arc<Inner>,
    key: usize,
}

impl Registration {
    pub async fn enable_dht(&self) {
        let mut state = self.inner.state.lock().await;
        let holder = &mut state.registry[self.key];
        holder.dht = self
            .inner
            .start_dht_lookup(repository_info_hash(holder.index.repository_id()));
    }

    pub async fn disable_dht(&self) {
        let mut state = self.inner.state.lock().await;
        state.registry[self.key].dht = None;
    }

    pub async fn is_dht_enabled(&self) -> bool {
        let state = self.inner.state.lock().await;
        state.registry[self.key].dht.is_some()
    }

    // Cancel the registration. Same as dropping self, but allows to wait until the deregistration
    // is complete.
    pub async fn cancel(self) {
        deregister(&self.inner, self.key).await
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let tasks = if let Some(tasks) = self.inner.tasks.upgrade() {
            tasks
        } else {
            return;
        };

        let inner = self.inner.clone();
        let key = self.key;

        // HACK: Can't run async code inside `drop`, spawning a task instead.
        tasks
            .other
            .spawn(async move { deregister(&inner, key).await });
    }
}

async fn deregister(inner: &Inner, key: usize) {
    let mut state = inner.state.lock().await;
    if let Some(holder) = state.registry.try_remove(key) {
        for broker in state.message_brokers.values_mut() {
            broker.destroy_link(holder.index.repository_id());
        }
    }
}

struct RegistrationHolder {
    index: Index,
    dht: Option<dht_discovery::LookupRequest>,
}

#[derive(Default)]
struct Tasks {
    local_discovery: BlockingMutex<Option<ScopedJoinHandle<()>>>,
    other: ScopedTaskSet,
}

struct Inner {
    listener_local_addr: SocketAddr,
    this_runtime_id: RuntimeId,
    state: Mutex<State>,
    dht_local_addrs: Option<IpStack<SocketAddr>>,
    dht_discovery: Option<DhtDiscovery>,
    dht_peer_found_tx: mpsc::UnboundedSender<SocketAddr>,
    connection_deduplicator: ConnectionDeduplicator,
    on_protocol_mismatch_tx: watch::Sender<Option<()>>,
    on_protocol_mismatch_rx: watch::Receiver<Option<()>>,
    // Note that unwrapping the upgraded weak pointer should be fine because if the underlying Arc
    // was Dropped, we would not be asking for the upgrade in the first place.
    tasks: Weak<Tasks>,
}

struct State {
    message_brokers: HashMap<RuntimeId, MessageBroker>,
    registry: Slab<RegistrationHolder>,
}

impl State {
    fn create_link(&mut self, index: Index) {
        for broker in self.message_brokers.values_mut() {
            broker.create_link(index.clone())
        }
    }
}

impl Inner {
    async fn enable_local_discovery(self: &Arc<Self>, enable: bool) {
        let tasks = self.tasks.upgrade().unwrap();
        let mut local_discovery = tasks.local_discovery.lock().unwrap();

        if !enable {
            *local_discovery = None;
            return;
        }

        if local_discovery.is_some() {
            return;
        }

        let port = self.listener_local_addr.port();
        *local_discovery = Some(scoped_task::spawn(self.clone().run_local_discovery(port)));
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
            let tasks = self.tasks.upgrade().unwrap();

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
            }
        }
    }

    fn start_dht_lookup(&self, info_hash: InfoHash) -> Option<dht_discovery::LookupRequest> {
        self.dht_discovery
            .as_ref()
            .map(|dht| dht.lookup(info_hash, self.dht_peer_found_tx.clone()))
    }

    fn establish_user_provided_connection(self: Arc<Self>, addr: SocketAddr) {
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

        let that_runtime_id =
            match perform_handshake(&mut stream, VERSION, self.this_runtime_id).await {
                Ok(writer_id) => writer_id,
                Err(error @ HandshakeError::ProtocolVersionMismatch) => {
                    log::error!("Failed to perform handshake with {}: {}", addr, error);
                    self.on_protocol_mismatch_tx.send(Some(())).unwrap_or(());
                    return;
                }
                Err(HandshakeError::Fatal(error)) => {
                    log::error!("Failed to perform handshake with {}: {}", addr, error);
                    return;
                }
            };

        // prevent self-connections.
        if that_runtime_id == self.this_runtime_id {
            log::debug!("Connection from self, discarding");
            return;
        }

        let released = permit.released();

        let mut state_guard = self.state.lock().await;
        let state = &mut *state_guard;

        match state.message_brokers.entry(that_runtime_id) {
            Entry::Occupied(entry) => entry.get().add_connection(stream, permit),
            Entry::Vacant(entry) => {
                log::info!("Connected to replica {:?} {}", that_runtime_id, addr);

                let mut broker =
                    MessageBroker::new(self.this_runtime_id, that_runtime_id, stream, permit);

                // TODO: for DHT connection we should only link the repository for which we did the
                // lookup but make sure we correctly handle edge cases, for example, when we have
                // more than one repository shared with the peer.
                for (_, holder) in &state.registry {
                    broker.create_link(holder.index.clone());
                }

                entry.insert(broker);
            }
        };

        drop(state_guard);

        released.notified().await;
        log::info!(
            "Lost {} TCP connection: {:?} {}",
            peer_source,
            that_runtime_id,
            addr
        );

        // Remove the broker if it has no more connections.
        let mut state = self.state.lock().await;
        if let Entry::Occupied(entry) = state.message_brokers.entry(that_runtime_id) {
            if !entry.get().has_connections() {
                entry.remove();
            }
        }
    }

    fn spawn<Fut>(&self, f: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        // TODO: this `unwrap` is sketchy. Maybe we should simply not spawn if `tasks` can't be
        // upgraded?
        self.tasks.upgrade().unwrap().other.spawn(f)
    }
}

// Exchange runtime ids with the peer. Returns their runtime id.
async fn perform_handshake(
    stream: &mut TcpStream,
    this_version: Version,
    this_runtime_id: RuntimeId,
) -> Result<RuntimeId, HandshakeError> {
    this_version.write_into(stream).await?;
    this_runtime_id.write_into(stream).await?;

    let that_version = Version::read_from(stream).await?;

    if that_version > this_version {
        return Err(HandshakeError::ProtocolVersionMismatch);
    }

    Ok(RuntimeId::read_from(stream).await?)
}

#[derive(Debug, Error)]
enum HandshakeError {
    #[error("protocol version mismatch")]
    ProtocolVersionMismatch,
    #[error("fatal error")]
    Fatal(#[from] io::Error),
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

#[derive(Debug, Error)]
#[error("network error")]
pub struct NetworkError(#[from] io::Error);

impl From<NetworkError> for Error {
    fn from(src: NetworkError) -> Self {
        Self::Network(src.0)
    }
}

pub fn repository_info_hash(id: &RepositoryId) -> InfoHash {
    // Calculate the info hash by hashing the id with SHA3-256 and taking the first 20 bytes.
    // (bittorrent uses SHA-1 but that is less secure).
    // `unwrap` is OK because the byte slice has the correct length.
    InfoHash::try_from(&id.salted_hash(b"ouisync repository info-hash").as_ref()[..INFO_HASH_LEN])
        .unwrap()
}
