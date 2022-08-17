mod barrier;
mod client;
mod config_keys;
mod connection;
mod crypto;
pub mod dht_discovery;
mod ip;
mod keep_alive;
mod local_discovery;
mod message;
mod message_broker;
mod message_dispatcher;
mod message_io;
mod options;
pub mod peer_addr;
mod peer_source;
mod protocol;
mod quic;
mod raw;
mod request;
mod runtime_id;
mod seen_peers;
mod server;
mod socket;
#[cfg(test)]
mod tests;
mod upnp;

pub use self::options::NetworkOptions;
use self::{
    connection::{
        ConnectionDeduplicator, ConnectionDirection, ConnectionPermit, PeerInfo, ReserveResult,
    },
    dht_discovery::DhtDiscovery,
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    peer_addr::{PeerAddr, PeerPort},
    peer_source::PeerSource,
    protocol::{Version, MAGIC, VERSION},
    runtime_id::{PublicRuntimeId, SecretRuntimeId},
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    config::ConfigStore,
    error::Error,
    repository::RepositoryId,
    scoped_task::{self, ScopedJoinHandle, ScopedTaskSet},
    state_monitor::StateMonitor,
    store::Store,
    sync::uninitialized_watch,
};
use async_trait::async_trait;
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use btdht::{self, InfoHash, INFO_HASH_LEN};
use futures_util::FutureExt;
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    future::Future,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex as BlockingMutex, Weak},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
    task, time,
};
use tracing::{field, instrument, Span};

pub struct Network {
    inner: Arc<Inner>,
    pub monitor: StateMonitor,
    // We keep tasks here instead of in Inner because we want them to be
    // destroyed when Network is Dropped.
    _tasks: Arc<Tasks>,
    _port_forwarder: Option<upnp::PortForwarder>,
}

impl Network {
    pub async fn new(
        options: &NetworkOptions,
        config: ConfigStore,
        monitor: StateMonitor,
    ) -> Result<Self, NetworkError> {
        let (quic_connector_v4, quic_listener_v4, udp_socket_v4) =
            if let Some(addr) = options.listen_quic_addr_v4() {
                Self::bind_quic_listener(addr, &config)
                    .await
                    .map(|(connector, acceptor, side_channel)| {
                        (Some(connector), Some(acceptor), Some(side_channel))
                    })
                    .unwrap_or((None, None, None))
            } else {
                (None, None, None)
            };

        let (quic_connector_v6, quic_listener_v6, udp_socket_v6) =
            if let Some(addr) = options.listen_quic_addr_v6() {
                Self::bind_quic_listener(addr, &config)
                    .await
                    .map(|(connector, acceptor, side_channel)| {
                        (Some(connector), Some(acceptor), Some(side_channel))
                    })
                    .unwrap_or((None, None, None))
            } else {
                (None, None, None)
            };

        let (tcp_listener_v4, tcp_listener_local_addr_v4) =
            if let Some(addr) = options.listen_tcp_addr_v4() {
                Self::bind_tcp_listener(addr, &config)
                    .await
                    .map(|(listener, addr)| (Some(listener), Some(addr)))
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

        let (tcp_listener_v6, tcp_listener_local_addr_v6) =
            if let Some(addr) = options.listen_tcp_addr_v6() {
                Self::bind_tcp_listener(addr, &config)
                    .await
                    .map(|(listener, addr)| (Some(listener), Some(addr)))
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

        let quic_listener_local_addr_v4 = quic_listener_v4.as_ref().map(|l| *l.local_addr());
        let quic_listener_local_addr_v6 = quic_listener_v6.as_ref().map(|l| *l.local_addr());

        let hole_puncher_v4 = udp_socket_v4.as_ref().map(|s| s.create_sender());
        let hole_puncher_v6 = udp_socket_v6.as_ref().map(|s| s.create_sender());

        let dht_discovery = if !options.disable_dht {
            // Also note that we're now only using quic for the transport discovered over the dht.
            // This is because the dht doesn't let us specify whether the remote peer SocketAddr is
            // TCP, UDP or anything else.
            // TODO: There are ways to address this: e.g. we could try both, or we could include
            // the protocol information in the info-hash generation. There are pros and cons to
            // these approaches.

            let monitor = monitor.make_child("DhtDiscovery");

            Some(DhtDiscovery::new(udp_socket_v4, udp_socket_v6, monitor).await)
        } else {
            None
        };

        let dht_local_addr_v4 = dht_discovery
            .as_ref()
            .and_then(|d| d.local_addr_v4())
            .cloned();

        let dht_local_addr_v6 = dht_discovery
            .as_ref()
            .and_then(|d| d.local_addr_v6())
            .cloned();

        let (port_forwarder, tcp_port_map, quic_port_map, dht_port_map) = if !options.disable_upnp {
            let port_forwarder = upnp::PortForwarder::new(monitor.make_child("UPnP"));

            // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
            // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.

            let tcp_port_map = tcp_listener_local_addr_v4.map(|addr| {
                port_forwarder.add_mapping(
                    addr.port(), // internal
                    addr.port(), // external
                    ip::Protocol::Tcp,
                )
            });

            let quic_port_map = quic_listener_local_addr_v4.map(|addr| {
                port_forwarder.add_mapping(
                    addr.port(), // internal
                    addr.port(), // external
                    ip::Protocol::Udp,
                )
            });

            if tcp_port_map.is_some() || quic_port_map.is_some() {
                let dht_port_map = dht_local_addr_v4.map(|addr| {
                    port_forwarder.add_mapping(
                        addr.port(), // internal
                        addr.port(), // external
                        ip::Protocol::Udp,
                    )
                });

                (
                    Some(port_forwarder),
                    tcp_port_map,
                    quic_port_map,
                    dht_port_map,
                )
            } else {
                (None, None, None, None)
            }
        } else {
            (None, None, None, None)
        };

        let tasks = Arc::new(Tasks::default());

        let (dht_peer_found_tx, mut dht_peer_found_rx) = mpsc::unbounded_channel();

        let (on_protocol_mismatch_tx, on_protocol_mismatch_rx) = uninitialized_watch::channel();

        let user_provided_peers = SeenPeers::new();

        let inner = Arc::new(Inner {
            monitor: monitor.clone(),
            quic_connector_v4,
            quic_connector_v6,
            quic_listener_local_addr_v4,
            quic_listener_local_addr_v6,
            tcp_listener_local_addr_v4,
            tcp_listener_local_addr_v6,
            hole_puncher_v4,
            hole_puncher_v6,
            this_runtime_id: SecretRuntimeId::generate(),
            state: BlockingMutex::new(State {
                message_brokers: HashMap::new(),
                registry: Slab::new(),
            }),
            _tcp_port_map: tcp_port_map,
            _quic_port_map: quic_port_map,
            _dht_port_map: dht_port_map,
            dht_local_addr_v4,
            dht_local_addr_v6,
            dht_discovery,
            dht_peer_found_tx,
            connection_deduplicator: ConnectionDeduplicator::new(),
            on_protocol_mismatch_tx,
            on_protocol_mismatch_rx,
            user_provided_peers,
            tasks: Arc::downgrade(&tasks),
            highest_seen_protocol_version: BlockingMutex::new(VERSION),
            our_addresses: BlockingMutex::new(HashSet::new()),
        });

        let network = Self {
            inner: inner.clone(),
            monitor,
            _tasks: tasks,
            _port_forwarder: port_forwarder,
        };

        // Start waiting for peers found by DhtDiscovery.
        // The spawned task gets destroyed once dht_peer_found_tx is destroyed.
        task::spawn({
            let weak = Arc::downgrade(&inner);
            async move {
                while let Some(seen_peer) = dht_peer_found_rx.recv().await {
                    if let Some(inner) = weak.upgrade() {
                        inner.spawn(inner.clone().handle_peer_found(seen_peer, PeerSource::Dht));
                    }
                }
            }
        });

        for listener in [tcp_listener_v4, tcp_listener_v6].into_iter().flatten() {
            inner.spawn(inner.clone().run_tcp_listener(listener));
        }

        for listener in [quic_listener_v4, quic_listener_v6].into_iter().flatten() {
            inner.spawn(inner.clone().run_quic_listener(listener));
        }

        inner
            .enable_local_discovery(!options.disable_local_discovery)
            .await;

        for peer in &options.peers {
            if let Some(seen_peer) = inner.user_provided_peers.insert(*peer) {
                inner.clone().establish_user_provided_connection(seen_peer);
            }
        }

        Ok(network)
    }

    pub fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.inner.tcp_listener_local_addr_v4.as_ref()
    }

    pub fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.inner.tcp_listener_local_addr_v6.as_ref()
    }

    pub fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.inner.quic_listener_local_addr_v4.as_ref()
    }

    pub fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.inner.quic_listener_local_addr_v6.as_ref()
    }

    pub fn dht_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.inner.dht_local_addr_v4.as_ref()
    }

    pub fn dht_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.inner.dht_local_addr_v6.as_ref()
    }

    pub fn handle(&self) -> Handle {
        Handle {
            inner: self.inner.clone(),
        }
    }

    pub fn collect_peer_info(&self) -> Vec<PeerInfo> {
        self.inner.connection_deduplicator.collect_peer_info()
    }

    // If the user did not specify (through NetworkOptions) the preferred port, then try to use
    // the one used last time. If that fails, or if this is the first time the app is running,
    // then use a random port.
    async fn bind_tcp_listener(
        preferred_addr: SocketAddr,
        config: &ConfigStore,
    ) -> Option<(TcpListener, SocketAddr)> {
        let (proto, config_key) = match preferred_addr {
            SocketAddr::V4(_) => ("IPv4", config_keys::LAST_USED_TCP_V4_PORT_KEY),
            SocketAddr::V6(_) => ("IPv6", config_keys::LAST_USED_TCP_V6_PORT_KEY),
        };

        match socket::bind::<TcpListener>(preferred_addr, config.entry(config_key)).await {
            Ok(listener) => match listener.local_addr() {
                Ok(addr) => {
                    tracing::info!("Configured {} TCP listener on {:?}", proto, addr);
                    Some((listener, addr))
                }
                Err(err) => {
                    tracing::warn!(
                        "Failed to get an address of {} TCP listener: {:?}",
                        proto,
                        err
                    );
                    None
                }
            },
            Err(err) => {
                tracing::warn!(
                    "Failed to bind listener to {} TCP address {:?}: {:?}",
                    proto,
                    preferred_addr,
                    err
                );
                None
            }
        }
    }

    async fn bind_quic_listener(
        preferred_addr: SocketAddr,
        config: &ConfigStore,
    ) -> Option<(quic::Connector, quic::Acceptor, quic::SideChannel)> {
        let (proto, config_key) = match preferred_addr {
            SocketAddr::V4(_) => ("IPv4", config_keys::LAST_USED_UDP_PORT_V4_KEY),
            SocketAddr::V6(_) => ("IPv6", config_keys::LAST_USED_UDP_PORT_V6_KEY),
        };

        let socket = match socket::bind::<UdpSocket>(preferred_addr, config.entry(config_key)).await
        {
            Ok(socket) => socket,
            Err(err) => {
                tracing::error!(
                    "Failed to bind {} QUIC socket to {:?}: {:?}",
                    proto,
                    preferred_addr,
                    err
                );
                return None;
            }
        };

        let socket = match socket.into_std() {
            Ok(socket) => socket,
            Err(err) => {
                tracing::error!(
                    "Failed to convert {} tokio::UdpSocket into std::UdpSocket for QUIC: {:?}",
                    proto,
                    err
                );
                return None;
            }
        };

        match quic::configure(socket) {
            Ok((connector, listener, side_channel)) => {
                tracing::info!(
                    "Configured {} QUIC stack on {:?}",
                    proto,
                    listener.local_addr()
                );
                Some((connector, listener, side_channel))
            }
            Err(e) => {
                tracing::warn!("Failed to configure {} QUIC stack: {}", proto, e);
                None
            }
        }
    }

    pub fn current_protocol_version(&self) -> u32 {
        VERSION.into()
    }
    pub fn highest_seen_protocol_version(&self) -> u32 {
        (*self.inner.highest_seen_protocol_version.lock().unwrap()).into()
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
    pub fn register(&self, store: Store) -> Registration {
        // TODO: consider disabling DHT by default, for privacy reasons.
        let dht = self
            .inner
            .start_dht_lookup(repository_info_hash(store.index.repository_id()));

        let mut network_state = self.inner.state.lock().unwrap();

        let key = network_state.registry.insert(RegistrationHolder {
            store: store.clone(),
            dht,
        });

        network_state.create_link(store);

        Registration {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Subscribe to network protocol mismatch events.
    pub fn on_protocol_mismatch(&self) -> uninitialized_watch::Receiver<()> {
        self.inner.on_protocol_mismatch_rx.clone()
    }

    /// Subscribe change in connected peers events.
    pub fn on_peer_set_change(&self) -> uninitialized_watch::Receiver<()> {
        self.inner.connection_deduplicator.on_change()
    }
}

pub struct Registration {
    inner: Arc<Inner>,
    key: usize,
}

impl Registration {
    pub fn enable_dht(&self) {
        let mut state = self.inner.state.lock().unwrap();
        let holder = &mut state.registry[self.key];
        holder.dht = self
            .inner
            .start_dht_lookup(repository_info_hash(holder.store.index.repository_id()));
    }

    pub fn disable_dht(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.registry[self.key].dht = None;
    }

    pub fn is_dht_enabled(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.registry[self.key].dht.is_some()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();

        if let Some(holder) = state.registry.try_remove(self.key) {
            for broker in state.message_brokers.values_mut() {
                broker.destroy_link(holder.store.index.repository_id());
            }
        }
    }
}

struct RegistrationHolder {
    store: Store,
    dht: Option<dht_discovery::LookupRequest>,
}

#[derive(Default)]
struct Tasks {
    local_discovery: BlockingMutex<Option<ScopedJoinHandle<()>>>,
    other: ScopedTaskSet,
}

struct Inner {
    monitor: StateMonitor,
    quic_connector_v4: Option<quic::Connector>,
    quic_connector_v6: Option<quic::Connector>,
    quic_listener_local_addr_v4: Option<SocketAddr>,
    quic_listener_local_addr_v6: Option<SocketAddr>,
    tcp_listener_local_addr_v4: Option<SocketAddr>,
    tcp_listener_local_addr_v6: Option<SocketAddr>,
    hole_puncher_v4: Option<quic::SideChannelSender>,
    hole_puncher_v6: Option<quic::SideChannelSender>,
    this_runtime_id: SecretRuntimeId,
    state: BlockingMutex<State>,
    _tcp_port_map: Option<upnp::Mapping>,
    _quic_port_map: Option<upnp::Mapping>,
    _dht_port_map: Option<upnp::Mapping>,
    dht_local_addr_v4: Option<SocketAddr>,
    dht_local_addr_v6: Option<SocketAddr>,
    dht_discovery: Option<DhtDiscovery>,
    dht_peer_found_tx: mpsc::UnboundedSender<SeenPeer>,
    connection_deduplicator: ConnectionDeduplicator,
    on_protocol_mismatch_tx: uninitialized_watch::Sender<()>,
    on_protocol_mismatch_rx: uninitialized_watch::Receiver<()>,
    user_provided_peers: SeenPeers,
    // Note that unwrapping the upgraded weak pointer should be fine because if the underlying Arc
    // was Dropped, we would not be asking for the upgrade in the first place.
    tasks: Weak<Tasks>,
    highest_seen_protocol_version: BlockingMutex<Version>,
    // Used to prevent repeatedly connecting to self.
    our_addresses: BlockingMutex<HashSet<PeerAddr>>,
}

struct State {
    message_brokers: HashMap<PublicRuntimeId, MessageBroker>,
    registry: Slab<RegistrationHolder>,
}

impl State {
    fn create_link(&mut self, store: Store) {
        for broker in self.message_brokers.values_mut() {
            broker.create_link(store.clone())
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

        let tcp_port = self
            .tcp_listener_local_addr_v4
            .as_ref()
            .map(|addr| PeerPort::Tcp(addr.port()));
        let quic_port = self
            .quic_listener_local_addr_v4
            .as_ref()
            .map(|addr| PeerPort::Quic(addr.port()));

        // Arbitrary order of preference.
        // TODO: Should we support all available?
        let port = tcp_port.or(quic_port);

        if let Some(port) = port {
            *local_discovery = Some(scoped_task::spawn(self.clone().run_local_discovery(port)));
        } else {
            tracing::error!(
                "Failed to enable local discovery because we don't have an IPv4 listener"
            );
        }
    }

    async fn run_local_discovery(self: Arc<Self>, listener_port: PeerPort) {
        let monitor = self.monitor.make_child("LocalDiscovery");

        let discovery = match LocalDiscovery::new(listener_port, monitor) {
            Ok(discovery) => discovery,
            Err(error) => {
                tracing::error!("Failed to create LocalDiscovery: {}", error);
                return;
            }
        };

        while let Some(peer) = discovery.recv().await {
            self.spawn(
                self.clone()
                    .handle_peer_found(peer, PeerSource::LocalDiscovery),
            )
        }
    }

    async fn run_tcp_listener(self: Arc<Self>, listener: TcpListener) {
        loop {
            let (socket, addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(error) => {
                    tracing::error!("Failed to accept incoming TCP connection: {}", error);
                    break;
                }
            };

            if let ReserveResult::Permit(permit) = self.connection_deduplicator.reserve(
                PeerAddr::Tcp(addr),
                PeerSource::Listener,
                ConnectionDirection::Incoming,
            ) {
                self.spawn(
                    self.clone()
                        .handle_new_connection(
                            raw::Stream::Tcp(socket),
                            PeerSource::Listener,
                            permit,
                        )
                        .map(|_| ()),
                )
            }
        }
    }

    async fn run_quic_listener(self: Arc<Self>, mut listener: quic::Acceptor) {
        loop {
            let socket = match listener.accept().await {
                Ok(socket) => socket,
                Err(error) => {
                    tracing::error!("Failed to accept incoming QUIC connection: {}", error);
                    break;
                }
            };

            if let ReserveResult::Permit(permit) = self.connection_deduplicator.reserve(
                PeerAddr::Quic(*socket.remote_address()),
                PeerSource::Listener,
                ConnectionDirection::Incoming,
            ) {
                self.spawn(
                    self.clone()
                        .handle_new_connection(
                            raw::Stream::Quic(socket),
                            PeerSource::Listener,
                            permit,
                        )
                        .map(|_| ()),
                )
            }
        }
    }

    fn start_dht_lookup(&self, info_hash: InfoHash) -> Option<dht_discovery::LookupRequest> {
        self.dht_discovery
            .as_ref()
            .map(|dht| dht.lookup(info_hash, self.dht_peer_found_tx.clone()))
    }

    fn establish_user_provided_connection(self: Arc<Self>, peer: SeenPeer) {
        self.spawn(
            self.clone()
                .handle_peer_found(peer, PeerSource::UserProvided),
        )
    }

    async fn connect(&self, addr: PeerAddr) -> Result<raw::Stream, ConnectError> {
        match addr {
            PeerAddr::Tcp(addr) => TcpStream::connect(addr)
                .await
                .map(raw::Stream::Tcp)
                .map_err(ConnectError::Tcp),
            PeerAddr::Quic(addr) => {
                let connector = if addr.is_ipv4() {
                    &self.quic_connector_v4
                } else {
                    &self.quic_connector_v6
                };

                connector
                    .as_ref()
                    .ok_or(ConnectError::NoSuitableQuicConnector)?
                    .connect(addr)
                    .await
                    .map(raw::Stream::Quic)
                    .map_err(ConnectError::Quic)
            }
        }
    }

    async fn handle_peer_found(self: Arc<Self>, peer: SeenPeer, source: PeerSource) {
        loop {
            let addr = match peer.addr() {
                Some(addr) => *addr,
                None => return,
            };

            if self.our_addresses.lock().unwrap().contains(&addr) {
                // Don't connect to self.
                return;
            }

            let permit = match self.connection_deduplicator.reserve(
                addr,
                source,
                ConnectionDirection::Outgoing,
            ) {
                ReserveResult::Permit(permit) => permit,
                ReserveResult::Occupied(on_release, their_source) => {
                    if source == their_source {
                        // This is a duplicate from the same source, ignore it.
                        return;
                    }

                    // This is a duplicate from a different source, if the other source releases
                    // it, then we may want to try to keep hold of it.
                    on_release.await;
                    continue;
                }
            };

            permit.mark_as_connecting();

            let socket = match self.connect_with_retries(&peer, source).await {
                Some(socket) => socket,
                None => break,
            };

            if !self
                .clone()
                .handle_new_connection(socket, source, permit)
                .await
            {
                break;
            }
        }
    }

    async fn connect_with_retries(
        &self,
        peer: &SeenPeer,
        source: PeerSource,
    ) -> Option<raw::Stream> {
        if !Self::ok_to_connect(peer.addr()?.socket_addr(), source) {
            return None;
        }

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(200))
            .with_max_interval(Duration::from_secs(10))
            // We'll continue trying for as long as `peer.addr().is_some()`.
            .with_max_elapsed_time(None)
            .build();

        let _hole_punching_task = self.start_punching_holes(*peer.addr()?);

        loop {
            // Note: This needs to be probed each time the loop starts. When the `addr` fn returns
            // `None` that means whatever discovery mechanism (LocalDiscovery or DhtDiscovery)
            // found it is no longer seeing it.
            let addr = *peer.addr()?;

            match self.connect(addr).await.ok() {
                Some(socket) => {
                    return Some(socket);
                }
                None => {
                    tracing::warn!(
                        "Failed to create {} connection to address {:?}",
                        source,
                        addr,
                    );

                    match backoff.next_backoff() {
                        Some(duration) => {
                            time::sleep(duration).await;
                        }
                        // We set max elapsed time to None above.
                        None => unreachable!(),
                    }
                }
            }
        }
    }

    // Filter out some weird `SocketAddr`s. We don't want to connect to those.
    fn ok_to_connect(addr: &SocketAddr, source: PeerSource) -> bool {
        if addr.port() == 0 || addr.port() == 1 {
            return false;
        }

        match addr {
            SocketAddr::V4(addr) => {
                let ip_addr = addr.ip();
                if ip_addr.octets()[0] == 0 {
                    return false;
                }
                if ip::is_benchmarking(ip_addr)
                    || ip::is_reserved(ip_addr)
                    || ip_addr.is_broadcast()
                    || ip_addr.is_documentation()
                {
                    return false;
                }

                if source == PeerSource::Dht
                    && (ip_addr.is_private() || ip_addr.is_loopback() || ip_addr.is_link_local())
                {
                    return false;
                }
            }
            SocketAddr::V6(addr) => {
                let ip_addr = addr.ip();

                if ip_addr.is_multicast()
                    || ip_addr.is_unspecified()
                    || ip::is_documentation(ip_addr)
                {
                    return false;
                }

                if source == PeerSource::Dht
                    && (ip_addr.is_loopback()
                        || ip::is_unicast_link_local(ip_addr)
                        || ip::is_unique_local(ip_addr))
                {
                    return false;
                }
            }
        }

        true
    }

    fn start_punching_holes(&self, addr: PeerAddr) -> Option<scoped_task::ScopedJoinHandle<()>> {
        if !addr.is_quic() {
            return None;
        }

        if !ip::is_global(&addr.ip()) {
            return None;
        }

        use std::net::IpAddr;

        let sender = match addr.ip() {
            IpAddr::V4(_) => self.hole_puncher_v4.clone(),
            IpAddr::V6(_) => self.hole_puncher_v6.clone(),
        };

        sender.map(|sender| {
            scoped_task::spawn(async move {
                use rand::Rng;

                let addr = addr.socket_addr();
                loop {
                    let duration_ms = rand::thread_rng().gen_range(5_000..15_000);
                    // Sleep first because the `connect` function that is normally called right
                    // after this function will send a SYN packet right a way, so no need to do
                    // double work here.
                    time::sleep(Duration::from_millis(duration_ms)).await;
                    // TODO: Consider using something non-identifiable (random) but something that
                    // won't interfere with (will be ignored by) the quic and btdht protocols.
                    let msg = b"punch";
                    sender.send_to(msg, addr).await.map(|_| ()).unwrap_or(());
                }
            })
        })
    }

    fn on_protocol_mismatch(&self, their_version: Version) {
        // We know that `their_version` is higher than our version because otherwise this function
        // wouldn't get called, but let's double check.
        assert!(VERSION < their_version);

        let mut highest = self.highest_seen_protocol_version.lock().unwrap();

        if *highest < their_version {
            *highest = their_version;
            self.on_protocol_mismatch_tx.send(()).unwrap_or(());
        }
    }

    /// Return true iff the peer is suitable for reconnection.
    #[instrument(name = "connection", skip(self, stream, permit), fields(addr = ?permit.addr()))]
    async fn handle_new_connection(
        self: Arc<Self>,
        mut stream: raw::Stream,
        peer_source: PeerSource,
        permit: ConnectionPermit,
    ) -> bool {
        tracing::info!("connection established");

        permit.mark_as_handshaking();

        let that_runtime_id =
            match perform_handshake(&mut stream, VERSION, &self.this_runtime_id).await {
                Ok(writer_id) => writer_id,
                Err(HandshakeError::ProtocolVersionMismatch(their_version)) => {
                    self.on_protocol_mismatch(their_version);
                    return false;
                }
                Err(HandshakeError::BadMagic | HandshakeError::Fatal(_)) => return false,
            };

        // prevent self-connections.
        if that_runtime_id == self.this_runtime_id.public() {
            tracing::debug!("connection from self, discarding");
            self.our_addresses.lock().unwrap().insert(permit.addr());
            return false;
        }

        permit.mark_as_active();

        let released = permit.released();

        {
            let mut state = self.state.lock().unwrap();
            let state = &mut *state;

            match state.message_brokers.entry(that_runtime_id) {
                Entry::Occupied(entry) => entry.get().add_connection(stream, permit),
                Entry::Vacant(entry) => {
                    let mut broker = MessageBroker::new(
                        self.this_runtime_id.public(),
                        that_runtime_id,
                        stream,
                        permit,
                    );

                    // TODO: for DHT connection we should only link the repository for which we did the
                    // lookup but make sure we correctly handle edge cases, for example, when we have
                    // more than one repository shared with the peer.
                    for (_, holder) in &state.registry {
                        broker.create_link(holder.store.clone());
                    }

                    entry.insert(broker);
                }
            };
        }

        released.await;
        tracing::info!("connection lost");

        // Remove the broker if it has no more connections.
        let mut state = self.state.lock().unwrap();
        if let Entry::Occupied(entry) = state.message_brokers.entry(that_runtime_id) {
            if !entry.get().has_connections() {
                entry.remove();
            }
        }

        true
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

//------------------------------------------------------------------------------
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("TCP error")]
    Tcp(std::io::Error),
    #[error("QUIC error")]
    Quic(quic::Error),
    #[error("No corresponding QUIC connector")]
    NoSuitableQuicConnector,
}

//------------------------------------------------------------------------------

// Exchange runtime ids with the peer. Returns their (verified) runtime id.
#[instrument(
    level = "trace",
    skip_all,
    fields(
        this_version = ?this_version,
        that_version,
        this_runtime_id = ?this_runtime_id.as_public_key(),
        that_runtime_id
    ),
    ret
)]
async fn perform_handshake(
    stream: &mut raw::Stream,
    this_version: Version,
    this_runtime_id: &SecretRuntimeId,
) -> Result<PublicRuntimeId, HandshakeError> {
    stream.write_all(MAGIC).await?;

    this_version.write_into(stream).await?;

    let mut that_magic = [0; MAGIC.len()];
    stream.read_exact(&mut that_magic).await?;

    if MAGIC != &that_magic {
        return Err(HandshakeError::BadMagic);
    }

    let that_version = Version::read_from(stream).await?;
    Span::current().record("that_version", &field::debug(&that_version));

    if that_version > this_version {
        return Err(HandshakeError::ProtocolVersionMismatch(that_version));
    }

    let that_runtime_id = runtime_id::exchange(this_runtime_id, stream).await?;
    Span::current().record(
        "that_runtime_id",
        &field::debug(that_runtime_id.as_public_key()),
    );

    Ok(that_runtime_id)
}

#[derive(Debug, Error)]
enum HandshakeError {
    #[error("protocol version mismatch")]
    ProtocolVersionMismatch(Version),
    #[error("bad magic")]
    BadMagic,
    #[error("fatal error")]
    Fatal(#[from] io::Error),
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

#[async_trait]
impl btdht::SocketTrait for quic::SideChannel {
    async fn send_to(&self, buf: &[u8], target: &SocketAddr) -> io::Result<()> {
        self.send_to(buf, target).await
    }

    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.recv_from(buf).await
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.local_addr()
    }
}
