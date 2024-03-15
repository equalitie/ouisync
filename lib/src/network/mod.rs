pub mod dht_discovery;
pub mod peer_addr;

mod barrier;
mod choke;
mod client;
mod connection;
mod connection_monitor;
mod constants;
mod crypto;
mod debug_payload;
mod gateway;
mod interface;
mod ip;
mod keep_alive;
mod local_discovery;
mod message;
mod message_broker;
mod message_dispatcher;
mod message_io;
mod peer_exchange; // TODO: replace with v2
mod peer_info;
mod peer_source;
mod peer_state;
mod pending;
mod protocol;
mod raw;
mod runtime_id;
mod seen_peers;
mod server;
mod stun;
mod stun_server_list;
#[cfg(test)]
mod tests;
mod traffic_tracker;
mod upnp;

pub use self::{
    connection::PeerInfoCollector,
    peer_info::PeerInfo,
    peer_source::PeerSource,
    peer_state::PeerState,
    runtime_id::{PublicRuntimeId, SecretRuntimeId},
    traffic_tracker::TrafficStats,
};
pub use net::stun::NatBehavior;

use self::{
    connection::{ConnectionDeduplicator, ConnectionPermit, ReserveResult},
    connection_monitor::ConnectionMonitor,
    dht_discovery::{DhtContactsStoreTrait, DhtDiscovery},
    gateway::{Gateway, StackAddresses},
    local_discovery::LocalDiscovery,
    message_broker::MessageBroker,
    peer_addr::{PeerAddr, PeerPort},
    peer_exchange::{PexDiscovery, PexRepository},
    protocol::{Version, MAGIC, VERSION},
    seen_peers::{SeenPeer, SeenPeers},
    stun::StunClients,
    traffic_tracker::TrafficTracker,
};
use crate::{
    collections::{hash_map::Entry, HashMap, HashSet},
    repository::{RepositoryHandle, RepositoryId, Vault},
    sync::uninitialized_watch,
};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use btdht::{self, InfoHash, INFO_HASH_LEN};
use deadlock::BlockingMutex;
use scoped_task::ScopedAbortHandle;
use slab::Slab;
use state_monitor::StateMonitor;
use std::{
    future::Future,
    io, mem,
    net::{SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Weak},
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    task::{AbortHandle, JoinSet},
    time::Duration,
};
use tracing::{Instrument, Span};

const DHT_ENABLED: &str = "dht_enabled";
const PEX_ENABLED: &str = "pex_enabled";

pub struct Network {
    inner: Arc<Inner>,
    // We keep tasks here instead of in Inner because we want them to be
    // destroyed when Network is Dropped.
    _tasks: Arc<BlockingMutex<JoinSet<()>>>,
}

impl Network {
    #[allow(clippy::new_without_default)] // Default doesn't seem right for this
    pub fn new(
        dht_contacts: Option<Arc<dyn DhtContactsStoreTrait>>,
        monitor: StateMonitor,
    ) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(1);
        let gateway = Gateway::new(incoming_tx);

        // Note that we're now only using quic for the transport discovered over the dht.
        // This is because the dht doesn't let us specify whether the remote peer SocketAddr is
        // TCP, UDP or anything else.
        // TODO: There are ways to address this: e.g. we could try both, or we could include
        // the protocol information in the info-hash generation. There are pros and cons to
        // these approaches.
        let dht_discovery = DhtDiscovery::new(None, None, dht_contacts, monitor.make_child("DHT"));
        // TODO: do we need unbounded channel here?
        let (dht_discovery_tx, dht_discovery_rx) = mpsc::unbounded_channel();

        let port_forwarder = upnp::PortForwarder::new(monitor.make_child("UPnP"));

        let (pex_discovery_tx, pex_discovery_rx) = mpsc::channel(1);
        let pex_discovery = PexDiscovery::new(pex_discovery_tx);

        let (on_protocol_mismatch_tx, _) = uninitialized_watch::channel();

        let user_provided_peers = SeenPeers::new();

        let this_runtime_id = SecretRuntimeId::random();
        let this_runtime_id_public = this_runtime_id.public();

        let connections_monitor = monitor.make_child("Connections");
        let peers_monitor = monitor.make_child("Peers");

        let tasks = Arc::new(BlockingMutex::new(JoinSet::new()));

        let inner = Arc::new(Inner {
            main_monitor: monitor,
            connections_monitor,
            peers_monitor,
            traffic_tracker: TrafficTracker::new(),
            span: Span::current(),
            gateway,
            this_runtime_id,
            state: BlockingMutex::new(State {
                message_brokers: Some(HashMap::default()),
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
            pex_discovery,
            stun_clients: StunClients::new(),
            connection_deduplicator: ConnectionDeduplicator::new(),
            on_protocol_mismatch_tx,
            user_provided_peers,
            tasks: Arc::downgrade(&tasks),
            highest_seen_protocol_version: BlockingMutex::new(VERSION),
            our_addresses: BlockingMutex::new(HashSet::default()),
        });

        inner.spawn(inner.clone().handle_incoming_connections(incoming_rx));
        inner.spawn(inner.clone().run_dht(dht_discovery_rx));
        inner.spawn(inner.clone().run_peer_exchange(pex_discovery_rx));

        tracing::debug!(this_runtime_id = ?this_runtime_id_public.as_public_key(), "Network created");

        Self {
            inner,
            _tasks: tasks,
        }
    }

    /// Binds the network to the specified addresses.
    /// Rebinds if already bound. Unbinds and disables the network if `addrs` is empty.
    ///
    /// NOTE: currently at most one address per protocol (QUIC/TCP) and family (IPv4/IPv6) is used
    /// and the rest are ignored, but this might change in the future.
    pub async fn bind(&self, addrs: &[PeerAddr]) {
        self.inner.bind(addrs).await
    }

    pub fn listener_local_addrs(&self) -> Vec<PeerAddr> {
        self.inner.gateway.listener_local_addrs()
    }

    pub fn set_port_forwarding_enabled(&self, enabled: bool) {
        let mut state = self.inner.port_forwarder_state.lock().unwrap();

        if enabled {
            if state.is_enabled() {
                return;
            }

            state.enable(PortMappings::new(
                &self.inner.port_forwarder,
                &self.inner.gateway,
            ));
        } else {
            state.disable(DisableReason::Explicit);
        }
    }

    pub fn is_port_forwarding_enabled(&self) -> bool {
        self.inner.port_forwarder_state.lock().unwrap().is_enabled()
    }

    pub fn set_local_discovery_enabled(&self, enabled: bool) {
        let mut state = self.inner.local_discovery_state.lock().unwrap();

        if enabled {
            if state.is_enabled() {
                return;
            }

            if let Some(handle) = self.inner.spawn_local_discovery() {
                state.enable(handle.into());
            } else {
                state.disable(DisableReason::Implicit);
            }
        } else {
            state.disable(DisableReason::Explicit);
        }
    }

    pub fn is_local_discovery_enabled(&self) -> bool {
        self.inner
            .local_discovery_state
            .lock()
            .unwrap()
            .is_enabled()
    }

    /// Find out external address using the STUN protocol.
    /// Currently QUIC only.
    pub async fn external_addr_v4(&self) -> Option<SocketAddrV4> {
        self.inner.stun_clients.external_addr_v4().await
    }

    /// Find out external address using the STUN protocol.
    /// Currently QUIC only.
    pub async fn external_addr_v6(&self) -> Option<SocketAddrV6> {
        self.inner.stun_clients.external_addr_v6().await
    }

    /// Determine the behaviour of the NAT we are behind. Returns `None` on unknown.
    /// Currently IPv4 only.
    pub async fn nat_behavior(&self) -> Option<NatBehavior> {
        self.inner.stun_clients.nat_behavior().await
    }

    /// Get the network traffic stats (total bytes sent and received).
    pub fn traffic_stats(&self) -> TrafficStats {
        self.inner.traffic_tracker.get()
    }

    pub fn add_user_provided_peer(&self, peer: &PeerAddr) {
        self.inner.clone().establish_user_provided_connection(peer);
    }

    pub fn remove_user_provided_peer(&self, peer: &PeerAddr) {
        self.inner.user_provided_peers.remove(peer)
    }

    pub fn this_runtime_id(&self) -> PublicRuntimeId {
        self.inner.this_runtime_id.public()
    }

    pub fn peer_info_collector(&self) -> PeerInfoCollector {
        self.inner.connection_deduplicator.peer_info_collector()
    }

    pub fn peer_info(&self, addr: PeerAddr) -> Option<PeerInfo> {
        self.inner.connection_deduplicator.get_peer_info(addr)
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

    /// Register a local repository into the network. This links the repository with all matching
    /// repositories of currently connected remote replicas as well as any replicas connected in
    /// the future. The repository is automatically deregistered when the returned handle is
    /// dropped.
    pub async fn register(&self, handle: RepositoryHandle) -> Registration {
        *handle.vault.monitor.info_hash.get() =
            Some(repository_info_hash(handle.vault.repository_id()));

        let metadata = handle.vault.metadata();
        let dht_enabled = metadata
            .get(DHT_ENABLED)
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);
        let pex_enabled = metadata
            .get(PEX_ENABLED)
            .await
            .unwrap_or(Some(false))
            .unwrap_or(false);

        let dht = if dht_enabled {
            Some(
                self.inner
                    .start_dht_lookup(repository_info_hash(handle.vault.repository_id())),
            )
        } else {
            None
        };

        let pex = self.inner.pex_discovery.new_repository();
        pex.set_enabled(pex_enabled);

        let choke_manager = choke::Manager::new();

        let mut network_state = self.inner.state.lock().unwrap();

        network_state.create_link(handle.vault.clone(), &pex, &choke_manager);

        let key = network_state.registry.insert(RegistrationHolder {
            vault: handle.vault,
            dht,
            pex,
            choke_manager,
        });

        Registration {
            inner: self.inner.clone(),
            key,
        }
    }

    /// Gracefully disconnect from peers. Failing to call this function on app termination will
    /// cause the peers to not learn that we disconnected just now. They will still find out later
    /// once the keep-alive mechanism kicks in, but in the mean time we will not be able to
    /// reconnect (by starting the app again) because the remote peer will keep dropping new
    /// connections from us.
    pub async fn shutdown(&self) {
        // TODO: Would be a nice-to-have to also wait for all the spawned tasks here (e.g. dicovery
        // mechanisms).
        let mut message_brokers = {
            let mut state = self.inner.state.lock().unwrap();
            match state.message_brokers.take() {
                Some(brokers) => brokers,
                None => {
                    tracing::warn!("Network already shut down");
                    return;
                }
            }
        };

        shutdown_brokers(&mut message_brokers).await;
    }
}

pub struct Registration {
    inner: Arc<Inner>,
    key: usize,
}

impl Registration {
    pub async fn set_dht_enabled(&self, enabled: bool) {
        self.set_metadata_bool(DHT_ENABLED, enabled).await;

        let mut state = self.inner.state.lock().unwrap();
        let holder = &mut state.registry[self.key];

        if enabled {
            holder.dht = Some(
                self.inner
                    .start_dht_lookup(repository_info_hash(holder.vault.repository_id())),
            );
        } else {
            holder.dht = None;
        }
    }

    /// This function provides the information to the user whether DHT is enabled for this
    /// repository, not necessarily whether the DHT tasks are currently running. The subtle
    /// difference is in that this function should return true even in case e.g. the whole network
    /// is disabled.
    pub fn is_dht_enabled(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.registry[self.key].dht.is_some()
    }

    pub async fn set_pex_enabled(&self, enabled: bool) {
        self.set_metadata_bool(PEX_ENABLED, enabled).await;

        let state = self.inner.state.lock().unwrap();
        state.registry[self.key].pex.set_enabled(enabled);
    }

    pub fn is_pex_enabled(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.registry[self.key].pex.is_enabled()
    }

    async fn set_metadata_bool(&self, name: &str, value: bool) {
        let metadata = self.inner.state.lock().unwrap().registry[self.key]
            .vault
            .metadata();
        metadata.set(name, value).await.ok();
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap();

        if let Some(holder) = state.registry.try_remove(self.key) {
            if let Some(brokers) = &mut state.message_brokers {
                for broker in brokers.values_mut() {
                    broker.destroy_link(holder.vault.local_id);
                }
            }
        }
    }
}

struct RegistrationHolder {
    vault: Vault,
    dht: Option<dht_discovery::LookupRequest>,
    pex: PexRepository,
    choke_manager: choke::Manager,
}

struct Inner {
    main_monitor: StateMonitor,
    connections_monitor: StateMonitor,
    peers_monitor: StateMonitor,
    traffic_tracker: TrafficTracker,
    span: Span,
    gateway: Gateway,
    this_runtime_id: SecretRuntimeId,
    state: BlockingMutex<State>,
    port_forwarder: upnp::PortForwarder,
    port_forwarder_state: BlockingMutex<ComponentState<PortMappings>>,
    local_discovery_state: BlockingMutex<ComponentState<ScopedAbortHandle>>,
    dht_discovery: DhtDiscovery,
    dht_discovery_tx: mpsc::UnboundedSender<SeenPeer>,
    pex_discovery: PexDiscovery,
    stun_clients: StunClients,
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
    // This is None once the network calls shutdown.
    message_brokers: Option<HashMap<PublicRuntimeId, MessageBroker>>,
    registry: Slab<RegistrationHolder>,
}

impl State {
    fn create_link(&mut self, repo: Vault, pex: &PexRepository, choke_manager: &choke::Manager) {
        if let Some(brokers) = &mut self.message_brokers {
            for broker in brokers.values_mut() {
                broker.create_link(repo.clone(), pex, choke_manager)
            }
        }
    }
}

impl Inner {
    fn is_shutdown(&self) -> bool {
        self.state.lock().unwrap().message_brokers.is_none()
    }

    async fn bind(self: &Arc<Self>, bind: &[PeerAddr]) {
        let conn = Connectivity::infer(bind);

        let bind = StackAddresses::from(bind);

        // TODO: Would be preferable to only rebind those stacks that actually need rebinding.
        if !self.gateway.addresses().any_stack_needs_rebind(&bind) {
            return;
        }

        // Gateway
        let side_channel_makers = self.gateway.bind(&bind).instrument(self.span.clone()).await;

        let (side_channel_maker_v4, side_channel_maker_v6) = match conn {
            Connectivity::Full => side_channel_makers,
            Connectivity::LocalOnly | Connectivity::Disabled => (None, None),
        };

        // STUN
        self.stun_clients.rebind(
            side_channel_maker_v4.as_ref().map(|m| m.make()),
            side_channel_maker_v6.as_ref().map(|m| m.make()),
        );

        // DHT
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
            self.disconnect_all().await;
        }
    }

    // Disconnect from all currently connected peers, regardless of their source.
    async fn disconnect_all(&self) {
        let mut message_brokers = {
            let mut state = self.state.lock().unwrap();
            match &mut state.message_brokers {
                Some(brokers) => {
                    let mut new = HashMap::default();
                    std::mem::swap(brokers, &mut new);
                    new
                }
                None => return,
            }
        };

        shutdown_brokers(&mut message_brokers).await;
    }

    fn spawn_local_discovery(self: &Arc<Self>) -> Option<AbortHandle> {
        let addrs = self.gateway.listener_local_addrs();
        let tcp_port = addrs
            .iter()
            .find(|addr| matches!(addr, PeerAddr::Tcp(SocketAddr::V4(_))))
            .map(|addr| PeerPort::Tcp(addr.port()));
        let quic_port = addrs
            .iter()
            .find(|addr| matches!(addr, PeerAddr::Quic(SocketAddr::V4(_))))
            .map(|addr| PeerPort::Quic(addr.port()));

        // Arbitrary order of preference.
        // TODO: Should we support all available?
        let port = tcp_port.or(quic_port);

        if let Some(port) = port {
            Some(
                self.spawn(
                    self.clone()
                        .run_local_discovery(port)
                        .instrument(self.span.clone()),
                ),
            )
        } else {
            tracing::error!("Not enabling local discovery because there is no IPv4 listener");
            None
        }
    }

    async fn run_local_discovery(self: Arc<Self>, listener_port: PeerPort) {
        let mut discovery = LocalDiscovery::new(
            listener_port,
            self.main_monitor.make_child("LocalDiscovery"),
        );

        loop {
            let peer = discovery.recv().await;

            if self.is_shutdown() {
                break;
            }

            self.spawn(
                self.clone()
                    .handle_peer_found(peer, PeerSource::LocalDiscovery),
            );
        }
    }

    fn start_dht_lookup(&self, info_hash: InfoHash) -> dht_discovery::LookupRequest {
        self.dht_discovery
            .start_lookup(info_hash, self.dht_discovery_tx.clone())
    }

    async fn run_dht(self: Arc<Self>, mut discovery_rx: mpsc::UnboundedReceiver<SeenPeer>) {
        while let Some(seen_peer) = discovery_rx.recv().await {
            if self.is_shutdown() {
                break;
            }

            self.spawn(self.clone().handle_peer_found(seen_peer, PeerSource::Dht));
        }
    }

    async fn run_peer_exchange(self: Arc<Self>, mut discovery_rx: mpsc::Receiver<SeenPeer>) {
        while let Some(peer) = discovery_rx.recv().await {
            if self.is_shutdown() {
                break;
            }

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
            match self
                .connection_deduplicator
                .reserve(addr, PeerSource::Listener)
            {
                ReserveResult::Permit(permit) => {
                    if self.is_shutdown() {
                        break;
                    }

                    let this = self.clone();

                    let monitor = self.span.in_scope(|| {
                        ConnectionMonitor::new(
                            &self.connections_monitor,
                            &permit.addr(),
                            permit.source(),
                        )
                    });
                    monitor.mark_as_connecting(permit.id());

                    self.spawn(async move {
                        this.handle_connection(stream, permit, &monitor).await;
                    });
                }
                ReserveResult::Occupied(_, _their_source, permit_id) => {
                    tracing::debug!(?addr, ?permit_id, "dropping accepted duplicate connection");
                }
            }
        }
    }

    async fn handle_peer_found(self: Arc<Self>, peer: SeenPeer, source: PeerSource) {
        let monitor = self.span.in_scope(|| {
            ConnectionMonitor::new(&self.connections_monitor, peer.initial_addr(), source)
        });

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(100))
            .with_max_interval(Duration::from_secs(8))
            .with_max_elapsed_time(None)
            .build();

        let mut next_sleep = None;

        loop {
            monitor.start();

            // TODO: We should also check whether the user still wants to accept connections from
            // the given `source` (the preference may have changed in the mean time).

            if self.is_shutdown() {
                return;
            }

            let addr = match peer.addr_if_seen() {
                Some(addr) => *addr,
                None => return,
            };

            if self.our_addresses.lock().unwrap().contains(&addr) {
                // Don't connect to self.
                return;
            }

            if let Some(sleep) = next_sleep {
                tracing::debug!(parent: monitor.span(), "Next connection attempt in {:?}", sleep);
                tokio::time::sleep(sleep).await;
            }

            next_sleep = backoff.next_backoff();

            let permit = match self.connection_deduplicator.reserve(addr, source) {
                ReserveResult::Permit(permit) => permit,
                ReserveResult::Occupied(on_release, their_source, permit_id) => {
                    if source == their_source {
                        // This is a duplicate from the same source, ignore it.
                        return;
                    }

                    // This is a duplicate from a different source, if the other source releases
                    // it, then we may want to try to keep hold of it.
                    monitor.mark_as_awaiting_permit();
                    tracing::debug!(
                        parent: monitor.span(),
                        permit_id,
                        "Duplicate from different source - awaiting permit"
                    );

                    on_release.await;
                    continue;
                }
            };

            permit.mark_as_connecting();
            monitor.mark_as_connecting(permit.id());
            tracing::debug!(parent: monitor.span(), "Connecting");

            let socket = match self
                .gateway
                .connect_with_retries(&peer, source)
                .instrument(monitor.span().clone())
                .await
            {
                Some(socket) => socket,
                None => break,
            };

            if !self.handle_connection(socket, permit, &monitor).await {
                break;
            }
        }
    }

    /// Return true iff the peer is suitable for reconnection.
    async fn handle_connection(
        &self,
        mut stream: raw::Stream,
        permit: ConnectionPermit,
        monitor: &ConnectionMonitor,
    ) -> bool {
        tracing::debug!(parent: monitor.span(), "Handshaking");

        permit.mark_as_handshaking();
        monitor.mark_as_handshaking();

        let handshake_result = perform_handshake(&mut stream, VERSION, &self.this_runtime_id).await;

        if let Err(error) = &handshake_result {
            tracing::debug!(parent: monitor.span(), ?error, "Handshake failed");
        }

        let that_runtime_id = match handshake_result {
            Ok(writer_id) => writer_id,
            Err(HandshakeError::ProtocolVersionMismatch(their_version)) => {
                self.on_protocol_mismatch(their_version);
                return false;
            }
            Err(HandshakeError::Timeout | HandshakeError::BadMagic | HandshakeError::Fatal(_)) => {
                return false
            }
        };

        // prevent self-connections.
        if that_runtime_id == self.this_runtime_id.public() {
            tracing::debug!(parent: monitor.span(), "Connection from self, discarding");
            self.our_addresses.lock().unwrap().insert(permit.addr());
            return false;
        }

        permit.mark_as_active(that_runtime_id);
        monitor.mark_as_active(that_runtime_id);
        tracing::info!(parent: monitor.span(), "Connected");

        let released = permit.released();

        {
            let mut state = self.state.lock().unwrap();
            let state = &mut *state;

            let brokers = match &mut state.message_brokers {
                Some(brokers) => brokers,
                // Network has been shut down.
                None => return false,
            };

            match brokers.entry(that_runtime_id) {
                Entry::Occupied(entry) => entry.get().add_connection(stream, permit),
                Entry::Vacant(entry) => {
                    let monitor = self
                        .peers_monitor
                        .make_child(format!("{:?}", that_runtime_id.as_public_key()));

                    let mut broker = self.span.in_scope(|| {
                        MessageBroker::new(
                            self.this_runtime_id.public(),
                            that_runtime_id,
                            stream,
                            permit,
                            self.pex_discovery.new_peer(),
                            monitor,
                            self.traffic_tracker.clone(),
                        )
                    });

                    // TODO: for DHT connection we should only link the repository for which we did the
                    // lookup but make sure we correctly handle edge cases, for example, when we have
                    // more than one repository shared with the peer.
                    for (_, holder) in &state.registry {
                        broker.create_link(
                            holder.vault.clone(),
                            &holder.pex,
                            &holder.choke_manager,
                        );
                    }

                    entry.insert(broker);
                }
            };
        }

        let _remover = MessageBrokerEntryGuard {
            state: &self.state,
            that_runtime_id,
            monitor,
        };

        released.await;

        true
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
            .spawn(f)
    }
}

//------------------------------------------------------------------------------

// Exchange runtime ids with the peer. Returns their (verified) runtime id.
async fn perform_handshake(
    stream: &mut raw::Stream,
    this_version: Version,
    this_runtime_id: &SecretRuntimeId,
) -> Result<PublicRuntimeId, HandshakeError> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async move {
        stream.write_all(MAGIC).await?;

        this_version.write_into(stream).await?;

        let mut that_magic = [0; MAGIC.len()];
        stream.read_exact(&mut that_magic).await?;

        if MAGIC != &that_magic {
            return Err(HandshakeError::BadMagic);
        }

        let that_version = Version::read_from(stream).await?;
        if that_version > this_version {
            return Err(HandshakeError::ProtocolVersionMismatch(that_version));
        }

        let that_runtime_id = runtime_id::exchange(this_runtime_id, stream).await?;

        Ok(that_runtime_id)
    })
    .await;

    match result {
        Ok(subresult) => subresult,
        Err(_) => Err(HandshakeError::Timeout),
    }
}

#[derive(Debug, Error)]
enum HandshakeError {
    #[error("protocol version mismatch")]
    ProtocolVersionMismatch(Version),
    #[error("bad magic")]
    BadMagic,
    #[error("timeout")]
    Timeout,
    #[error("fatal error")]
    Fatal(#[from] io::Error),
}

// RAII guard which when dropped removes the broker from the network state if it has no connections.
struct MessageBrokerEntryGuard<'a> {
    state: &'a BlockingMutex<State>,
    that_runtime_id: PublicRuntimeId,
    monitor: &'a ConnectionMonitor,
}

impl Drop for MessageBrokerEntryGuard<'_> {
    fn drop(&mut self) {
        tracing::info!(parent: self.monitor.span(), "Disconnected");

        let mut state = self.state.lock().unwrap();
        if let Some(brokers) = &mut state.message_brokers {
            if let Entry::Occupied(entry) = brokers.entry(self.that_runtime_id) {
                if !entry.get().has_connections() {
                    entry.remove();
                }
            }
        }
    }
}

struct PortMappings {
    _mappings: Vec<upnp::Mapping>,
}

impl PortMappings {
    fn new(forwarder: &upnp::PortForwarder, gateway: &Gateway) -> Self {
        let mappings = gateway
            .listener_local_addrs()
            .into_iter()
            .filter_map(|addr| {
                match addr {
                    PeerAddr::Quic(SocketAddr::V4(addr)) => {
                        Some(forwarder.add_mapping(
                            addr.port(), // internal
                            addr.port(), // external
                            ip::Protocol::Udp,
                        ))
                    }
                    PeerAddr::Tcp(SocketAddr::V4(addr)) => {
                        Some(forwarder.add_mapping(
                            addr.port(), // internal
                            addr.port(), // external
                            ip::Protocol::Tcp,
                        ))
                    }
                    PeerAddr::Quic(SocketAddr::V6(_)) | PeerAddr::Tcp(SocketAddr::V6(_)) => {
                        // TODO: the ipv6 port typically doesn't need to be port-mapped but it might
                        // need to be opened in the firewall ("pinholed"). Consider using UPnP for that
                        // as well.
                        None
                    }
                }
            })
            .collect();

        Self {
            _mappings: mappings,
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

async fn shutdown_brokers(message_brokers: &mut HashMap<PublicRuntimeId, MessageBroker>) {
    let mut futures = Vec::with_capacity(message_brokers.len());

    for (_runtime_id, broker) in message_brokers.drain() {
        futures.push(async move {
            broker.shutdown().await;
        });
    }

    futures_util::future::join_all(futures).await;
}
