use super::{
    config_keys, ip, options::NetworkOptions, peer_addr::PeerAddr, peer_source::PeerSource, quic,
    raw, seen_peers::SeenPeer, socket, upnp,
};
use crate::{
    config::ConfigStore,
    scoped_task::{self, ScopedJoinHandle},
    state_monitor::StateMonitor,
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
    quic_v4: Option<QuicStack>,
    quic_v6: Option<QuicStack>,
    tcp_v4: Option<TcpStack>,
    tcp_v6: Option<TcpStack>,
    _port_forwarder: upnp::PortForwarder,
}

impl Gateway {
    /// `incoming_tx` is the sender for the incoming connections.
    ///
    /// Returns `Self` and the side channels for IPv4 and IPv6 respecitvely (if available)
    pub async fn new(
        options: &NetworkOptions,
        config: ConfigStore,
        monitor: StateMonitor,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> (Self, Option<quic::SideChannel>, Option<quic::SideChannel>) {
        let (mut quic_v4, side_channel_v4) = if let Some(addr) = options.listen_quic_addr_v4() {
            QuicStack::new(addr, &config, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let (quic_v6, side_channel_v6) = if let Some(addr) = options.listen_quic_addr_v6() {
            QuicStack::new(addr, &config, incoming_tx.clone())
                .await
                .map(|(stack, side_channel)| (Some(stack), Some(side_channel)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let mut tcp_v4 = if let Some(addr) = options.listen_tcp_addr_v4() {
            TcpStack::new(addr, &config, incoming_tx.clone()).await
        } else {
            None
        };

        let tcp_v6 = if let Some(addr) = options.listen_tcp_addr_v6() {
            TcpStack::new(addr, &config, incoming_tx).await
        } else {
            None
        };

        let port_forwarder = upnp::PortForwarder::new(monitor.make_child("UPnP"));

        if !options.disable_upnp {
            // TODO: the ipv6 port typically doesn't need to be port-mapped but it might need to
            // be opened in the firewall ("pinholed"). Consider using UPnP for that as well.
            if let Some(stack) = &mut quic_v4 {
                stack.enable_port_forwarding(&port_forwarder);
            }

            if let Some(stack) = &mut tcp_v4 {
                stack.enable_port_forwarding(&port_forwarder);
            }
        }

        let this = Self {
            quic_v4,
            quic_v6,
            tcp_v4,
            tcp_v6,
            _port_forwarder: port_forwarder,
        };

        (this, side_channel_v4, side_channel_v6)
    }

    pub fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.tcp_v4.as_ref().map(|stack| &stack.listener_local_addr)
    }

    pub fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.tcp_v6.as_ref().map(|stack| &stack.listener_local_addr)
    }

    pub fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.quic_v4
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    pub fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.quic_v6
            .as_ref()
            .map(|stack| &stack.listener_local_addr)
    }

    pub async fn connect_with_retries(
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

#[derive(Debug, Error)]
pub(super) enum ConnectError {
    #[error("TCP error")]
    Tcp(std::io::Error),
    #[error("QUIC error")]
    Quic(quic::Error),
    #[error("No corresponding QUIC connector")]
    NoSuitableQuicConnector,
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
    ) -> Option<(Self, quic::SideChannel)> {
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

        let (connector, listener, side_channel) = match quic::configure(socket) {
            Ok((connector, listener, side_channel)) => {
                tracing::info!(
                    "Configured {} QUIC stack on {:?}",
                    family,
                    listener.local_addr()
                );
                (connector, listener, side_channel)
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

        let hole_puncher = side_channel.create_sender();

        let this = Self {
            connector,
            listener_local_addr,
            _listener_task: listener_task,
            hole_puncher,
            port_mapping: None,
        };

        Some((this, side_channel))
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
