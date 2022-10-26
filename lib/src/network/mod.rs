mod barrier;
mod client;
mod config_keys;
mod connection;
mod crypto;
pub mod dht_discovery;
mod gateway;
mod interface;
mod ip;
mod keep_alive;
mod local_discovery;
mod message;
mod message_broker;
mod message_dispatcher;
mod message_io;
pub mod peer_addr;
mod peer_exchange;
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

use self::{
    connection::{ConnectionDeduplicator, ConnectionPermit, PeerInfo, ReserveResult},
    dht_discovery::DhtDiscovery,
    gateway::Gateway,
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    peer_addr::{PeerAddr, PeerPort},
    peer_exchange::{PexController, PexDiscovery, PexPayload},
    peer_source::PeerSource,
    protocol::{Version, MAGIC, VERSION},
    runtime_id::{PublicRuntimeId, SecretRuntimeId},
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    config::ConfigStore, repository::RepositoryId, scoped_task::ScopedAbortHandle,
    state_monitor::StateMonitor, store::Store, sync::uninitialized_watch,
};
use btdht::{self, InfoHash, INFO_HASH_LEN};
use futures_util::FutureExt;
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    future::Future,
    io, mem,
    net::SocketAddr,
    sync::{Arc, Mutex as BlockingMutex, Weak},
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    task::{AbortHandle, JoinSet},
};
use tracing::{field, instrument, Instrument, Span};

pub struct Network {
    inner: Arc<Inner>,
    pub monitor: StateMonitor,
    // We keep tasks here instead of in Inner because we want them to be
    // destroyed when Network is Dropped.
    _tasks: Arc<BlockingMutex<JoinSet<()>>>,
}

impl Network {
    pub fn new(config: ConfigStore, monitor: StateMonitor) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(1);
        let gateway = Gateway::new(config, incoming_tx);

        // Note that we're now only using quic for the transport discovered over the dht.
        // This is because the dht doesn't let us specify whether the remote peer SocketAddr is
        // TCP, UDP or anything else.
        // TODO: There are ways to address this: e.g. we could try both, or we could include
        // the protocol information in the info-hash generation. There are pros and cons to
        // these approaches.
        let dht_discovery = {
            let monitor = monitor.make_child("DhtDiscovery");
            DhtDiscovery::new(None, None, monitor)
        };

        let port_forwarder = {
            let monitor = monitor.make_child("UPnP");
            upnp::PortForwarder::new(monitor)
        };

        let tasks = Arc::new(BlockingMutex::new(JoinSet::new()));

        // TODO: do we need unbounded channel here?
        let (dht_discovery_tx, dht_discovery_rx) = mpsc::unbounded_channel();
        let (pex_discovery_tx, pex_discovery_rx) = mpsc::channel(1);

        let (on_protocol_mismatch_tx, _) = uninitialized_watch::channel();

        let user_provided_peers = SeenPeers::new();

        let inner = Arc::new(Inner {
            monitor: monitor.clone(),
            gateway,
            this_runtime_id: SecretRuntimeId::generate(),
            state: BlockingMutex::new(State {
                message_brokers: HashMap::new(),
                registry: Slab::new(),
            }),
            port_forwarder,
            port_forwarder_state: BlockingMutex::new(ComponentState::disabled(
                DisableReason::Explicit,
            )),
            local_discovery_state: BlockingMutex::new(ComponentState::disabled(
                DisableReason::Explicit,
            )),
            dht_discovery,
            dht_discovery_tx,
            pex_discovery_tx,
            connection_deduplicator: ConnectionDeduplicator::new(),
            on_protocol_mismatch_tx,
            user_provided_peers,
            tasks: Arc::downgrade(&tasks),
            highest_seen_protocol_version: BlockingMutex::new(VERSION),
            our_addresses: BlockingMutex::new(HashSet::new()),
        });

        inner.spawn(inner.clone().handle_incoming_connections(incoming_rx));
        inner.spawn(inner.clone().run_dht(dht_discovery_rx));
        inner.spawn(inner.clone().run_peer_exchange(pex_discovery_rx));

        Self {
            inner,
            monitor,
            _tasks: tasks,
        }
    }

    pub fn tcp_listener_local_addr_v4(&self) -> Option<SocketAddr> {
        self.inner.gateway.tcp_listener_local_addr_v4()
    }

    pub fn tcp_listener_local_addr_v6(&self) -> Option<SocketAddr> {
        self.inner.gateway.tcp_listener_local_addr_v6()
    }

    pub fn quic_listener_local_addr_v4(&self) -> Option<SocketAddr> {
        self.inner.gateway.quic_listener_local_addr_v4()
    }

    pub fn quic_listener_local_addr_v6(&self) -> Option<SocketAddr> {
        self.inner.gateway.quic_listener_local_addr_v6()
    }

    pub fn enable_port_forwarding(&self) {
        self.inner.enable_port_forwarding()
    }

    pub fn disable_port_forwarding(&self) {
        self.inner.disable_port_forwarding()
    }

    pub fn is_port_forwarding_enabled(&self) -> bool {
        self.inner.is_port_forwarding_enabled()
    }

    pub fn enable_local_discovery(&self) {
        let mut state = self.inner.local_discovery_state.lock().unwrap();

        if let Some(handle) = self.inner.spawn_local_discovery() {
            state.enable(handle.into());
        } else {
            state.disable(DisableReason::Implicit);
        }
    }

    pub fn disable_local_discovery(&self) {
        self.inner
            .local_discovery_state
            .lock()
            .unwrap()
            .disable(DisableReason::Explicit);
    }

    pub fn is_local_discovery_enabled(&self) -> bool {
        self.inner
            .local_discovery_state
            .lock()
            .unwrap()
            .is_enabled()
    }

    pub fn add_user_provided_peer(&self, peer: &PeerAddr) {
        self.inner.clone().establish_user_provided_connection(peer);
    }

    pub fn remove_user_provided_peer(&self, peer: &PeerAddr) {
        self.inner.user_provided_peers.remove(peer)
    }

    pub fn handle(&self) -> Handle {
        Handle {
            inner: self.inner.clone(),
        }
    }

    pub fn this_runtime_id(&self) -> PublicRuntimeId {
        self.inner.this_runtime_id.public()
    }

    pub fn collect_peer_info(&self) -> Vec<PeerInfo> {
        self.inner.connection_deduplicator.collect_peer_info()
    }

    pub fn knows_peer(&self, addr: PeerAddr) -> bool {
        self.inner.connection_deduplicator.contains(addr)
    }

    pub fn current_protocol_version(&self) -> u32 {
        VERSION.into()
    }

    pub fn highest_seen_protocol_version(&self) -> u32 {
        (*self.inner.highest_seen_protocol_version.lock().unwrap()).into()
    }

    /// Subscribe to network protocol mismatch events.
    pub fn on_protocol_mismatch(&self) -> uninitialized_watch::Receiver<()> {
        self.inner.on_protocol_mismatch_tx.subscribe()
    }

    /// Subscribe change in connected peers events.
    pub fn on_peer_set_change(&self) -> uninitialized_watch::Receiver<()> {
        self.inner.connection_deduplicator.on_change()
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
        let pex = PexController::new(
            self.inner.connection_deduplicator.on_change(),
            self.inner.pex_discovery_tx.clone(),
        );

        let mut network_state = self.inner.state.lock().unwrap();

        network_state.create_link(store.clone(), &pex);

        let key = network_state.registry.insert(RegistrationHolder {
            store,
            dht: None,
            pex,
        });

        Registration {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Binds the network to the specified addresses.
    /// Rebinds if already bound. Unbinds and disables the network if `addrs` is empty.
    pub async fn bind(&self, addrs: &[PeerAddr]) {
        self.inner.bind(addrs).await
    }

    /// Is the network enabled
    pub fn is_bound(&self) -> bool {
        self.inner.gateway.is_bound()
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
        holder.dht = Some(
            self.inner
                .start_dht_lookup(repository_info_hash(holder.store.index.repository_id())),
        );
    }

    pub fn disable_dht(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.registry[self.key].dht = None;
    }

    /// This function provides the information to the user whether DHT is enabled for this
    /// repository, not necessarily whether the DHT tasks are currently running. The subtle
    /// difference is in that this function should return true even in case e.g. the whole network
    /// is disabled.
    pub fn is_dht_enabled(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.registry[self.key].dht.is_some()
    }

    pub fn enable_pex(&self) {
        let state = self.inner.state.lock().unwrap();
        let holder = &state.registry[self.key];
        holder.pex.set_enabled(true);
    }

    pub fn disable_pex(&self) {
        let state = self.inner.state.lock().unwrap();
        let holder = &state.registry[self.key];
        holder.pex.set_enabled(false);
    }

    pub fn is_pex_enabled(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        let holder = &state.registry[self.key];
        holder.pex.is_enabled()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();

        if let Some(holder) = state.registry.try_remove(self.key) {
            for broker in state.message_brokers.values_mut() {
                broker.destroy_link(holder.store.local_id);
            }
        }
    }
}

struct RegistrationHolder {
    store: Store,
    dht: Option<dht_discovery::LookupRequest>,
    pex: PexController,
}

struct Inner {
    monitor: StateMonitor,
    gateway: Gateway,
    this_runtime_id: SecretRuntimeId,
    state: BlockingMutex<State>,
    port_forwarder: upnp::PortForwarder,
    port_forwarder_state: BlockingMutex<ComponentState<PortMappings>>,
    local_discovery_state: BlockingMutex<ComponentState<ScopedAbortHandle>>,
    dht_discovery: DhtDiscovery,
    dht_discovery_tx: mpsc::UnboundedSender<SeenPeer>,
    pex_discovery_tx: mpsc::Sender<PexPayload>,
    connection_deduplicator: ConnectionDeduplicator,
    on_protocol_mismatch_tx: uninitialized_watch::Sender<()>,
    user_provided_peers: SeenPeers,
    // Note that unwrapping the upgraded weak pointer should be fine because if the underlying Arc
    // was Dropped, we would not be asking for the upgrade in the first place.
    tasks: Weak<BlockingMutex<JoinSet<()>>>,
    highest_seen_protocol_version: BlockingMutex<Version>,
    // Used to prevent repeatedly connecting to self.
    our_addresses: BlockingMutex<HashSet<PeerAddr>>,
}

struct State {
    message_brokers: HashMap<PublicRuntimeId, MessageBroker>,
    registry: Slab<RegistrationHolder>,
}

impl State {
    fn create_link(&mut self, store: Store, pex: &PexController) {
        for broker in self.message_brokers.values_mut() {
            broker.create_link(store.clone(), pex)
        }
    }
}

impl Inner {
    async fn bind(self: &Arc<Self>, bind: &[PeerAddr]) {
        let conn = Connectivity::infer(bind);

        // Gateway
        let side_channel_makers = self.gateway.bind(bind).await;

        // DHT
        let (side_channel_maker_v4, side_channel_maker_v6) = match conn {
            Connectivity::Full => side_channel_makers,
            Connectivity::LocalOnly | Connectivity::Disabled => (None, None),
        };

        self.dht_discovery
            .rebind(side_channel_maker_v4, side_channel_maker_v6);

        // Port forwarding
        match conn {
            Connectivity::Full => {
                let mut state = self.port_forwarder_state.lock().unwrap();
                if !state.is_disabled(DisableReason::Explicit) {
                    state.enable(PortMappings::new(&self.port_forwarder, &self.gateway));
                }
            }
            Connectivity::LocalOnly | Connectivity::Disabled => {
                self.port_forwarder_state
                    .lock()
                    .unwrap()
                    .disable_if_enabled(DisableReason::Implicit);
            }
        }

        // Local discovery
        //
        // Note: no need to check the Connectivity because local discovery depends only on whether
        // Gateway is bound.
        {
            let mut state = self.local_discovery_state.lock().unwrap();
            if !state.is_disabled(DisableReason::Explicit) {
                if let Some(handle) = self.spawn_local_discovery() {
                    state.enable(handle.into());
                } else {
                    state.disable(DisableReason::Implicit);
                }
            }
        }

        // - If we are disabling connectivity, disconnect from all existing peers.
        // - If we are going from `Full` -> `LocalOnly`, also disconnect from all with the
        //   assumption that the local ones will be subsequently re-established. Ideally we would
        //   disconnect only the non-local ones to avoid the reconnect overhead, but the
        //   implementation is simpler this way and the trade-off doesn't seem to be too bad.
        // - If we are going to `Full`, keep all existing connections.
        if matches!(conn, Connectivity::LocalOnly | Connectivity::Disabled) {
            self.disconnect_all();
        }
    }

    // Disconnect from all currently connected peers, regardless of their source.
    fn disconnect_all(&self) {
        self.state.lock().unwrap().message_brokers.clear();
    }

    fn enable_port_forwarding(&self) {
        self.port_forwarder_state
            .lock()
            .unwrap()
            .enable(PortMappings::new(&self.port_forwarder, &self.gateway));
    }

    fn disable_port_forwarding(&self) {
        self.port_forwarder_state
            .lock()
            .unwrap()
            .disable(DisableReason::Explicit);
    }

    fn is_port_forwarding_enabled(&self) -> bool {
        self.port_forwarder_state.lock().unwrap().is_enabled()
    }

    fn spawn_local_discovery(self: &Arc<Self>) -> Option<AbortHandle> {
        let tcp_port = self
            .gateway
            .tcp_listener_local_addr_v4()
            .map(|addr| PeerPort::Tcp(addr.port()));
        let quic_port = self
            .gateway
            .quic_listener_local_addr_v4()
            .map(|addr| PeerPort::Quic(addr.port()));

        // Arbitrary order of preference.
        // TODO: Should we support all available?
        let port = tcp_port.or(quic_port);

        if let Some(port) = port {
            Some(self.spawn(self.clone().run_local_discovery(port)))
        } else {
            tracing::trace!("Not enabling local discovery because there is no IPv4 listener");
            None
        }
    }

    async fn run_local_discovery(self: Arc<Self>, listener_port: PeerPort) {
        let monitor = self.monitor.make_child("LocalDiscovery");

        let mut discovery = LocalDiscovery::new(listener_port, monitor);

        loop {
            let peer = discovery.recv().await;

            self.spawn(
                self.clone()
                    .handle_peer_found(peer, PeerSource::LocalDiscovery),
            );
        }
    }

    fn start_dht_lookup(&self, info_hash: InfoHash) -> dht_discovery::LookupRequest {
        self.dht_discovery
            .lookup(info_hash, self.dht_discovery_tx.clone())
    }

    async fn run_dht(self: Arc<Self>, mut discovery_rx: mpsc::UnboundedReceiver<SeenPeer>) {
        while let Some(seen_peer) = discovery_rx.recv().await {
            self.spawn(self.clone().handle_peer_found(seen_peer, PeerSource::Dht));
        }
    }

    async fn run_peer_exchange(self: Arc<Self>, discovery_rx: mpsc::Receiver<PexPayload>) {
        let mut discovery = PexDiscovery::new(discovery_rx);

        while let Some(peer) = discovery.recv().await {
            self.spawn(
                self.clone()
                    .handle_peer_found(peer, PeerSource::PeerExchange),
            );
        }
    }

    fn establish_user_provided_connection(self: Arc<Self>, peer: &PeerAddr) {
        let peer = match self.user_provided_peers.insert(*peer) {
            Some(peer) => peer,
            // Already in `user_provided_peers`.
            None => return,
        };

        self.spawn(
            self.clone()
                .handle_peer_found(peer, PeerSource::UserProvided),
        );
    }

    async fn handle_incoming_connections(
        self: Arc<Self>,
        mut rx: mpsc::Receiver<(raw::Stream, PeerAddr)>,
    ) {
        while let Some((stream, addr)) = rx.recv().await {
            if let ReserveResult::Permit(permit) = self
                .connection_deduplicator
                .reserve(addr, PeerSource::Listener)
            {
                self.spawn(
                    self.clone()
                        .handle_new_connection(stream, permit)
                        .map(|_| ()),
                );
            }
        }
    }

    #[instrument(skip_all, fields(?source, addr = ?peer.initial_addr()))]
    async fn handle_peer_found(self: Arc<Self>, peer: SeenPeer, source: PeerSource) {
        loop {
            let addr = match peer.addr_if_seen() {
                Some(addr) => *addr,
                None => return,
            };

            if self.our_addresses.lock().unwrap().contains(&addr) {
                // Don't connect to self.
                return;
            }

            let permit = match self.connection_deduplicator.reserve(addr, source) {
                ReserveResult::Permit(permit) => permit,
                ReserveResult::Occupied(on_release, their_source) => {
                    if source == their_source {
                        // This is a duplicate from the same source, ignore it.
                        return;
                    }

                    tracing::trace!(state = "awaiting permit");

                    // This is a duplicate from a different source, if the other source releases
                    // it, then we may want to try to keep hold of it.
                    on_release.recv().await;
                    continue;
                }
            };

            tracing::trace!(?addr, ?source, "peer found");

            permit.mark_as_connecting();

            tracing::trace!(state = "connecting", permit_id = permit.id());

            let socket = match self.gateway.connect_with_retries(&peer, source).await {
                Some(socket) => socket,
                None => break,
            };

            tracing::trace!(state = "handling");

            if !self.clone().handle_new_connection(socket, permit).await {
                break;
            }
        }
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
    #[instrument(
        name = "connection",
        parent = None,
        skip_all,
        fields(
            addr = ?permit.addr(),
            permit_id = permit.id(),
        )
    )]
    async fn handle_new_connection(
        self: Arc<Self>,
        mut stream: raw::Stream,
        permit: ConnectionPermit,
    ) -> bool {
        tracing::info!("connection established");

        permit.mark_as_handshaking();

        tracing::trace!(state = "handshaking");

        let that_runtime_id =
            match perform_handshake(&mut stream, VERSION, &self.this_runtime_id).await {
                Ok(writer_id) => writer_id,
                Err(HandshakeError::ProtocolVersionMismatch(their_version)) => {
                    self.on_protocol_mismatch(their_version);
                    return false;
                }
                Err(HandshakeError::BadMagic | HandshakeError::Fatal(_)) => return false,
            };

        tracing::trace!(that_runtime_id = ?that_runtime_id.as_public_key());

        // prevent self-connections.
        if that_runtime_id == self.this_runtime_id.public() {
            tracing::debug!("connection from self, discarding");
            self.our_addresses.lock().unwrap().insert(permit.addr());
            return false;
        }

        permit.mark_as_active(that_runtime_id);

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
                        broker.create_link(holder.store.clone(), &holder.pex);
                    }

                    entry.insert(broker);
                }
            };
        }

        let _remover = MessageBrokerEntryGuard {
            state: &self.state,
            that_runtime_id,
        };

        tracing::trace!(state = "awaiting message broker release");

        released.recv().await;
        tracing::info!("connection lost");

        true
    }

    fn spawn<Fut>(&self, f: Fut) -> AbortHandle
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.tasks
            .upgrade()
            // TODO: this `unwrap` is sketchy. Maybe we should simply not spawn if `tasks` can't be
            // upgraded?
            .unwrap()
            .lock()
            .unwrap()
            // Preserve the current span across spawns to track the parent-child relation.
            .spawn(f.instrument(Span::current()))
    }
}

//------------------------------------------------------------------------------

// Exchange runtime ids with the peer. Returns their (verified) runtime id.
#[instrument(
    skip_all,
    fields(
        this_version = ?this_version,
        that_version,
        this_runtime_id = ?this_runtime_id.as_public_key(),
        that_runtime_id
    ),
    err(Debug)
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

    tracing::trace!("handshake complete");

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

// RAII guard which when dropped removes the broker from the network state if it has no connections.
struct MessageBrokerEntryGuard<'a> {
    state: &'a BlockingMutex<State>,
    that_runtime_id: PublicRuntimeId,
}

impl Drop for MessageBrokerEntryGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Entry::Occupied(entry) = state.message_brokers.entry(self.that_runtime_id) {
            if !entry.get().has_connections() {
                entry.remove();
            }
        }
    }
}

struct PortMappings {
    _tcp_v4: Option<upnp::Mapping>,
    _quic_v4: Option<upnp::Mapping>,
}

impl PortMappings {
    fn new(forwarder: &upnp::PortForwarder, gateway: &Gateway) -> Self {
        let tcp_v4 = gateway.tcp_listener_local_addr_v4().map(|addr| {
            forwarder.add_mapping(
                addr.port(), // internal
                addr.port(), // external
                ip::Protocol::Tcp,
            )
        });

        let quic_v4 = gateway.quic_listener_local_addr_v4().map(|addr| {
            forwarder.add_mapping(
                addr.port(), // internal
                addr.port(), // external
                ip::Protocol::Udp,
            )
        });

        // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
        // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.

        Self {
            _tcp_v4: tcp_v4,
            _quic_v4: quic_v4,
        }
    }
}

enum ComponentState<T> {
    Enabled(T),
    Disabled(DisableReason),
}

impl<T> ComponentState<T> {
    fn disabled(reason: DisableReason) -> Self {
        Self::Disabled(reason)
    }

    fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    fn is_disabled(&self, reason: DisableReason) -> bool {
        match self {
            Self::Disabled(current_reason) if *current_reason == reason => true,
            Self::Disabled(_) | Self::Enabled(_) => false,
        }
    }

    fn disable(&mut self, reason: DisableReason) -> Option<T> {
        match mem::replace(self, Self::Disabled(reason)) {
            Self::Enabled(payload) => Some(payload),
            Self::Disabled(_) => None,
        }
    }

    fn disable_if_enabled(&mut self, reason: DisableReason) -> Option<T> {
        match self {
            Self::Enabled(_) => match mem::replace(self, Self::Disabled(reason)) {
                Self::Enabled(payload) => Some(payload),
                Self::Disabled(_) => unreachable!(),
            },
            Self::Disabled(_) => None,
        }
    }

    fn enable(&mut self, payload: T) -> Option<T> {
        match mem::replace(self, Self::Enabled(payload)) {
            Self::Enabled(payload) => Some(payload),
            Self::Disabled(_) => None,
        }
    }
}

#[derive(Eq, PartialEq)]
enum DisableReason {
    // Disabled implicitly because `Network` was disabled
    Implicit,
    // Disabled explicitly
    Explicit,
}

enum Connectivity {
    Disabled,
    LocalOnly,
    Full,
}

impl Connectivity {
    fn infer(addrs: &[PeerAddr]) -> Self {
        if addrs.is_empty() {
            return Self::Disabled;
        }

        let global = addrs
            .iter()
            .map(|addr| addr.ip())
            .any(|ip| ip.is_unspecified() || ip::is_global(&ip));

        if global {
            Self::Full
        } else {
            Self::LocalOnly
        }
    }
}

pub fn repository_info_hash(id: &RepositoryId) -> InfoHash {
    // Calculate the info hash by hashing the id with SHA3-256 and taking the first 20 bytes.
    // (bittorrent uses SHA-1 but that is less secure).
    // `unwrap` is OK because the byte slice has the correct length.
    InfoHash::try_from(&id.salted_hash(b"ouisync repository info-hash").as_ref()[..INFO_HASH_LEN])
        .unwrap()
}

fn instrument_task<F>(task: F) -> tracing::instrument::Instrumented<F>
where
    F: Future,
{
    task.instrument(tracing::info_span!("spawn"))
}
