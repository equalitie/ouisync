use super::{
    config_keys, ip, options::NetworkOptions, peer_addr::PeerAddr, peer_source::PeerSource, quic,
    raw, seen_peers::SeenPeer, socket, upnp,
};
use crate::{
    config::ConfigStore,
    scoped_task::{self, ScopedJoinHandle},
    state_monitor::StateMonitor,
    sync::atomic_slot::AtomicSlot,
};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    select,
    sync::mpsc,
    time,
};
use tracing::Instrument;

/// Established incomming and outgoing connections.
pub(super) struct Gateway {
    state: AtomicSlot<State>,
    config: ConfigStore,
    port_forwarder: upnp::PortForwarder,
}

impl Gateway {
    /// Create a new `Gateway` that is initially disabled.
    ///
    /// `incoming_tx` is the sender for the incoming connections.
    pub fn new(
        options: &NetworkOptions,
        config: ConfigStore,
        monitor: StateMonitor,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Self {
        let state = Disabled::new(&options.bind, !options.disable_upnp, incoming_tx);
        let state = State::Disabled(state);
        let state = AtomicSlot::new(state);

        let port_forwarder = upnp::PortForwarder::new(monitor.make_child("UPnP"));

        Self {
            state,
            config,
            port_forwarder,
        }
    }

    pub fn tcp_listener_local_addr_v4(&self) -> Option<SocketAddr> {
        self.state.read().tcp_listener_local_addr_v4().copied()
    }

    pub fn tcp_listener_local_addr_v6(&self) -> Option<SocketAddr> {
        self.state.read().tcp_listener_local_addr_v6().copied()
    }

    pub fn quic_listener_local_addr_v4(&self) -> Option<SocketAddr> {
        self.state.read().quic_listener_local_addr_v4().copied()
    }

    pub fn quic_listener_local_addr_v6(&self) -> Option<SocketAddr> {
        self.state.read().quic_listener_local_addr_v6().copied()
    }

    /// Enables all listeners and connectors. If the state actually transitioned from disabled to
    /// enabled (that is, it wasn't already enabled), returns the IPv4 and IPv6 side channels
    /// makers respectively, if available. Otherwise returns pair of `None`s.
    pub async fn enable(
        &self,
    ) -> (
        Option<quic::SideChannelMaker>,
        Option<quic::SideChannelMaker>,
    ) {
        let mut current_state = self.state.read();

        loop {
            let disabled = match &*current_state {
                State::Enabled(_) => break (None, None),
                State::Disabled(disabled) => disabled,
            };

            let (enabled, side_channel_maker_v4, side_channel_maker_v6) = disabled
                .to_enabled(&self.config, &self.port_forwarder)
                .await;
            let next_state = State::Enabled(enabled);

            match self.state.compare_and_swap(&current_state, next_state) {
                Ok(_) => break (side_channel_maker_v4, side_channel_maker_v6),
                Err((prev_state, _)) => current_state = prev_state,
            }
        }
    }

    /// Disables all listeners/connectors
    pub fn disable(&self) {
        let mut current_state = self.state.read();

        loop {
            let enabled = match &*current_state {
                State::Enabled(enabled) => enabled,
                State::Disabled(_) => break,
            };

            let disabled = enabled.to_disabled();
            let next_state = State::Disabled(disabled);

            match self.state.compare_and_swap(&current_state, next_state) {
                Ok(_) => break,
                Err((prev_state, _)) => current_state = prev_state,
            }
        }
    }

    /// Checks whether this `Gateway` is enabled
    pub fn is_enabled(&self) -> bool {
        matches!(*self.state.read(), State::Enabled(_))
    }

    pub async fn connect_with_retries(
        &self,
        peer: &SeenPeer,
        source: PeerSource,
    ) -> Option<raw::Stream> {
        let state = self.state.read();
        match &*state {
            State::Enabled(state) => state.connect_with_retries(peer, source).await,
            State::Disabled(_) => None,
        }
    }
}

#[derive(Debug, Error)]
pub(super) enum ConnectError {
    #[error("TCP error")]
    Tcp(std::io::Error),
    #[error("QUIC error")]
    Quic(quic::Error),
    #[error("No corresponding QUIC connector")]
    NoSuitableQuicConnector,
}

enum State {
    Enabled(Enabled),
    Disabled(Disabled),
}

struct Enabled {
    quic_v4: Option<QuicStack>,
    quic_v6: Option<QuicStack>,
    tcp_v4: Option<TcpStack>,
    tcp_v6: Option<TcpStack>,
    incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
}

struct Disabled {
    quic_listener_local_addr_v4: Option<SocketAddr>,
    quic_listener_local_addr_v6: Option<SocketAddr>,
    tcp_listener_local_addr_v4: Option<SocketAddr>,
    tcp_listener_local_addr_v6: Option<SocketAddr>,
    port_forwarding_enabled: bool,
    incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
}

impl State {
    fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        match self {
            Self::Enabled(state) => state.quic_listener_local_addr_v4(),
            Self::Disabled(state) => state.quic_listener_local_addr_v4.as_ref(),
        }
    }

    fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        match self {
            Self::Enabled(state) => state.quic_listener_local_addr_v6(),
            Self::Disabled(state) => state.quic_listener_local_addr_v6.as_ref(),
        }
    }

    fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        match self {
            Self::Enabled(state) => state.tcp_listener_local_addr_v4(),
            Self::Disabled(state) => state.tcp_listener_local_addr_v4.as_ref(),
        }
    }

    fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        match self {
            Self::Enabled(state) => state.tcp_listener_local_addr_v6(),
            Self::Disabled(state) => state.tcp_listener_local_addr_v6.as_ref(),
        }
    }
}

impl Enabled {
    fn to_disabled(&self) -> Disabled {
        let port_forwarding_enabled =
            self.quic_port_forwarding_enabled() || self.tcp_port_forwarding_enabled();

        Disabled {
            quic_listener_local_addr_v4: self.quic_listener_local_addr_v4().copied(),
            quic_listener_local_addr_v6: self.quic_listener_local_addr_v6().copied(),
            tcp_listener_local_addr_v4: self.tcp_listener_local_addr_v4().copied(),
            tcp_listener_local_addr_v6: self.tcp_listener_local_addr_v6().copied(),
            port_forwarding_enabled,
            incoming_tx: self.incoming_tx.clone(),
        }
    }

    fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.quic_v4
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.quic_v6
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.tcp_v4.as_ref().map(|stack| &stack.listener_local_addr)
    }

    fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.tcp_v6.as_ref().map(|stack| &stack.listener_local_addr)
    }

    fn quic_port_forwarding_enabled(&self) -> bool {
        self.quic_v4
            .as_ref()
            .map(|stack| stack.port_mapping.is_some())
            .unwrap_or(false)
    }

    fn tcp_port_forwarding_enabled(&self) -> bool {
        self.tcp_v4
            .as_ref()
            .map(|stack| stack.port_mapping.is_some())
            .unwrap_or(false)
    }

    async fn connect_with_retries(
        &self,
        peer: &SeenPeer,
        source: PeerSource,
    ) -> Option<raw::Stream> {
        if !ok_to_connect(peer.addr()?.socket_addr(), source) {
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

            match self.connect(addr).await {
                Ok(socket) => {
                    return Some(socket);
                }
                Err(error) => {
                    tracing::warn!(
                        "Failed to create {} connection to address {:?}: {:?}",
                        source,
                        addr,
                        error
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

    async fn connect(&self, addr: PeerAddr) -> Result<raw::Stream, ConnectError> {
        match addr {
            PeerAddr::Tcp(addr) => TcpStream::connect(addr)
                .await
                .map(raw::Stream::Tcp)
                .map_err(ConnectError::Tcp),
            PeerAddr::Quic(addr) => {
                let stack = self
                    .quic_stack_for(&addr.ip())
                    .ok_or(ConnectError::NoSuitableQuicConnector)?;

                stack
                    .connector
                    .connect(addr)
                    .await
                    .map(raw::Stream::Quic)
                    .map_err(ConnectError::Quic)
            }
        }
    }

    fn start_punching_holes(&self, addr: PeerAddr) -> Option<scoped_task::ScopedJoinHandle<()>> {
        if !addr.is_quic() {
            return None;
        }

        if !ip::is_global(&addr.ip()) {
            return None;
        }

        let stack = self.quic_stack_for(&addr.ip())?;
        let sender = stack.hole_puncher.clone();
        let task = scoped_task::spawn(async move {
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
        });

        Some(task)
    }

    fn quic_stack_for(&self, ip: &IpAddr) -> Option<&QuicStack> {
        match ip {
            IpAddr::V4(_) => self.quic_v4.as_ref(),
            IpAddr::V6(_) => self.quic_v6.as_ref(),
        }
    }
}

impl Disabled {
    fn new(
        bind: &[PeerAddr],
        port_forwarding_enabled: bool,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Self {
        let quic_listener_local_addr_v4 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });

        let quic_listener_local_addr_v6 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Quic(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });
        let tcp_listener_local_addr_v4 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V4(_)) => Some(*addr),
            _ => None,
        });
        let tcp_listener_local_addr_v6 = bind.iter().find_map(|addr| match addr {
            PeerAddr::Tcp(addr @ SocketAddr::V6(_)) => Some(*addr),
            _ => None,
        });

        Self {
            quic_listener_local_addr_v4,
            quic_listener_local_addr_v6,
            tcp_listener_local_addr_v4,
            tcp_listener_local_addr_v6,
            port_forwarding_enabled,
            incoming_tx,
        }
    }

    async fn to_enabled(
        &self,
        config: &ConfigStore,
        port_forwarder: &upnp::PortForwarder,
    ) -> (
        Enabled,
        Option<quic::SideChannelMaker>,
        Option<quic::SideChannelMaker>,
    ) {
        let (mut quic_v4, side_channel_maker_v4) =
            if let Some(addr) = self.quic_listener_local_addr_v4 {
                QuicStack::new(addr, config, self.incoming_tx.clone())
                    .await
                    .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

        let (quic_v6, side_channel_maker_v6) = if let Some(addr) = self.quic_listener_local_addr_v6
        {
            QuicStack::new(addr, config, self.incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let mut tcp_v4 = if let Some(addr) = self.tcp_listener_local_addr_v4 {
            TcpStack::new(addr, config, self.incoming_tx.clone()).await
        } else {
            None
        };

        let tcp_v6 = if let Some(addr) = self.tcp_listener_local_addr_v6 {
            TcpStack::new(addr, config, self.incoming_tx.clone()).await
        } else {
            None
        };

        if self.port_forwarding_enabled {
            // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
            // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.

            if let Some(stack) = &mut quic_v4 {
                stack.enable_port_forwarding(port_forwarder);
            }

            if let Some(stack) = &mut tcp_v4 {
                stack.enable_port_forwarding(port_forwarder);
            }
        }

        let enabled = Enabled {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
            incoming_tx: self.incoming_tx.clone(),
        };

        (enabled, side_channel_maker_v4, side_channel_maker_v6)
    }
}

struct QuicStack {
    connector: quic::Connector,
    listener_local_addr: SocketAddr,
    _listener_task: ScopedJoinHandle<()>,
    hole_puncher: quic::SideChannelSender,
    port_mapping: Option<upnp::Mapping>,
}

impl QuicStack {
    async fn new(
        preferred_addr: SocketAddr,
        config: &ConfigStore,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Option<(Self, quic::SideChannelMaker)> {
        let (family, config_key) = match preferred_addr {
            SocketAddr::V4(_) => ("IPv4", config_keys::LAST_USED_UDP_PORT_V4_KEY),
            SocketAddr::V6(_) => ("IPv6", config_keys::LAST_USED_UDP_PORT_V6_KEY),
        };

        let socket = match socket::bind::<UdpSocket>(preferred_addr, config.entry(config_key)).await
        {
            Ok(socket) => socket,
            Err(err) => {
                tracing::error!(
                    "Failed to bind {} QUIC socket to {:?}: {:?}",
                    family,
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
                    family,
                    err
                );
                return None;
            }
        };

        let (connector, listener, side_channel_maker) = match quic::configure(socket) {
            Ok((connector, listener, side_channel_maker)) => {
                tracing::info!(
                    "Configured {} QUIC stack on {:?}",
                    family,
                    listener.local_addr()
                );
                (connector, listener, side_channel_maker)
            }
            Err(e) => {
                tracing::warn!("Failed to configure {} QUIC stack: {}", family, e);
                return None;
            }
        };

        let listener_local_addr = *listener.local_addr();
        let listener_task = scoped_task::spawn(
            run_quic_listener(listener, incoming_tx).instrument(tracing::info_span!(
                "listener",
                proto = "QUIC",
                family
            )),
        );

        let hole_puncher = side_channel_maker.make().sender();

        let this = Self {
            connector,
            listener_local_addr,
            _listener_task: listener_task,
            hole_puncher,
            port_mapping: None,
        };

        Some((this, side_channel_maker))
    }

    fn enable_port_forwarding(&mut self, forwarder: &upnp::PortForwarder) {
        self.port_mapping = Some(forwarder.add_mapping(
            self.listener_local_addr.port(), // internal
            self.listener_local_addr.port(), // external
            ip::Protocol::Udp,
        ));
    }
}

struct TcpStack {
    listener_local_addr: SocketAddr,
    _listener_task: ScopedJoinHandle<()>,
    port_mapping: Option<upnp::Mapping>,
}

impl TcpStack {
    // If the user did not specify (through NetworkOptions) the preferred port, then try to use
    // the one used last time. If that fails, or if this is the first time the app is running,
    // then use a random port.
    async fn new(
        preferred_addr: SocketAddr,
        config: &ConfigStore,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> Option<Self> {
        let (family, config_key) = match preferred_addr {
            SocketAddr::V4(_) => ("IPv4", config_keys::LAST_USED_TCP_V4_PORT_KEY),
            SocketAddr::V6(_) => ("IPv6", config_keys::LAST_USED_TCP_V6_PORT_KEY),
        };

        let listener =
            match socket::bind::<TcpListener>(preferred_addr, config.entry(config_key)).await {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::warn!(
                        "Failed to bind listener to {} TCP address {:?}: {:?}",
                        family,
                        preferred_addr,
                        err
                    );
                    return None;
                }
            };

        let listener_local_addr = match listener.local_addr() {
            Ok(addr) => {
                tracing::info!("Configured {} TCP listener on {:?}", family, addr);
                addr
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to get local address of {} TCP listener: {:?}",
                    family,
                    err
                );
                return None;
            }
        };

        let listener_task = scoped_task::spawn(
            run_tcp_listener(listener, incoming_tx).instrument(tracing::info_span!(
                "listener",
                proto = "TCP",
                family
            )),
        );

        Some(Self {
            listener_local_addr,
            _listener_task: listener_task,
            port_mapping: None,
        })
    }

    fn enable_port_forwarding(&mut self, forwarder: &upnp::PortForwarder) {
        self.port_mapping = Some(forwarder.add_mapping(
            self.listener_local_addr.port(), // internal
            self.listener_local_addr.port(), // external
            ip::Protocol::Tcp,
        ));
    }
}

async fn run_tcp_listener(listener: TcpListener, tx: mpsc::Sender<(raw::Stream, PeerAddr)>) {
    loop {
        let result = select! {
            result = listener.accept() => result,
            _ = tx.closed() => break,
        };

        match result {
            Ok((stream, addr)) => {
                tx.send((raw::Stream::Tcp(stream), PeerAddr::Tcp(addr)))
                    .await
                    .ok();
            }
            Err(error) => {
                tracing::error!("Failed to accept incoming TCP connection: {}", error);
                break;
            }
        }
    }
}

async fn run_quic_listener(
    mut listener: quic::Acceptor,
    tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
) {
    loop {
        let result = select! {
            result = listener.accept() => result,
            _ = tx.closed() => break,
        };

        match result {
            Ok(socket) => {
                let addr = *socket.remote_address();
                tx.send((raw::Stream::Quic(socket), PeerAddr::Quic(addr)))
                    .await
                    .ok();
            }
            Err(error) => {
                tracing::error!("Failed to accept incoming QUIC connection: {}", error);
                break;
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

            if ip_addr.is_multicast() || ip_addr.is_unspecified() || ip::is_documentation(ip_addr) {
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
