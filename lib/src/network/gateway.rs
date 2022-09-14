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
use std::{net::SocketAddr, time::Duration};
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
    quic_connector_v4: Option<quic::Connector>,
    quic_connector_v6: Option<quic::Connector>,
    quic_listener_local_addr_v4: Option<SocketAddr>,
    quic_listener_local_addr_v6: Option<SocketAddr>,
    _quic_listener_task_v4: Option<ScopedJoinHandle<()>>,
    _quic_listener_task_v6: Option<ScopedJoinHandle<()>>,
    tcp_listener_local_addr_v4: Option<SocketAddr>,
    tcp_listener_local_addr_v6: Option<SocketAddr>,
    _tcp_listener_task_v4: Option<ScopedJoinHandle<()>>,
    _tcp_listener_task_v6: Option<ScopedJoinHandle<()>>,
    hole_puncher_v4: Option<quic::SideChannelSender>,
    hole_puncher_v6: Option<quic::SideChannelSender>,
    _port_forwarder: Option<upnp::PortForwarder>,
    _tcp_port_map: Option<upnp::Mapping>,
    _quic_port_map: Option<upnp::Mapping>,
}

impl Gateway {
    /// `incoming_tx` is the sender for the incoming connections.
    ///
    /// Apart from `Self` also returns the side channels for IPv4 and IPv6 respecitvely (if
    /// available)
    pub async fn new(
        options: &NetworkOptions,
        config: ConfigStore,
        monitor: StateMonitor,
        incoming_tx: mpsc::Sender<(raw::Stream, PeerAddr)>,
    ) -> (Self, Option<quic::SideChannel>, Option<quic::SideChannel>) {
        let (quic_connector_v4, quic_listener_v4, udp_socket_v4) =
            if let Some(addr) = options.listen_quic_addr_v4() {
                configure_quic(addr, &config)
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
                configure_quic(addr, &config)
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
                configure_tcp(addr, &config)
                    .await
                    .map(|(listener, addr)| (Some(listener), Some(addr)))
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };

        let (tcp_listener_v6, tcp_listener_local_addr_v6) =
            if let Some(addr) = options.listen_tcp_addr_v6() {
                configure_tcp(addr, &config)
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

        let (port_forwarder, tcp_port_map, quic_port_map) = if !options.disable_upnp {
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
                (Some(port_forwarder), tcp_port_map, quic_port_map)
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

        let quic_listener_task_v4 = quic_listener_v4.map(|listener| {
            scoped_task::spawn(
                run_quic_listener(listener, incoming_tx.clone())
                    .instrument(tracing::info_span!("quic_listener_v4")),
            )
        });

        let quic_listener_task_v6 = quic_listener_v6.map(|listener| {
            scoped_task::spawn(
                run_quic_listener(listener, incoming_tx.clone())
                    .instrument(tracing::info_span!("quic_listener_v6")),
            )
        });

        let tcp_listener_task_v4 = tcp_listener_v4.map(|listener| {
            scoped_task::spawn(
                run_tcp_listener(listener, incoming_tx.clone())
                    .instrument(tracing::info_span!("tcp_listener_v4")),
            )
        });

        let tcp_listener_task_v6 = tcp_listener_v6.map(|listener| {
            scoped_task::spawn(
                run_tcp_listener(listener, incoming_tx.clone())
                    .instrument(tracing::info_span!("tcp_listener_v6")),
            )
        });

        let this = Self {
            quic_connector_v4,
            quic_connector_v6,
            quic_listener_local_addr_v4,
            quic_listener_local_addr_v6,
            _quic_listener_task_v4: quic_listener_task_v4,
            _quic_listener_task_v6: quic_listener_task_v6,
            tcp_listener_local_addr_v4,
            tcp_listener_local_addr_v6,
            _tcp_listener_task_v4: tcp_listener_task_v4,
            _tcp_listener_task_v6: tcp_listener_task_v6,
            hole_puncher_v4,
            hole_puncher_v6,
            _port_forwarder: port_forwarder,
            _tcp_port_map: tcp_port_map,
            _quic_port_map: quic_port_map,
        };

        (this, udp_socket_v4, udp_socket_v6)
    }

    pub fn tcp_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.tcp_listener_local_addr_v4.as_ref()
    }

    pub fn tcp_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.tcp_listener_local_addr_v6.as_ref()
    }

    pub fn quic_listener_local_addr_v4(&self) -> Option<&SocketAddr> {
        self.quic_listener_local_addr_v4.as_ref()
    }

    pub fn quic_listener_local_addr_v6(&self) -> Option<&SocketAddr> {
        self.quic_listener_local_addr_v6.as_ref()
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

// If the user did not specify (through NetworkOptions) the preferred port, then try to use
// the one used last time. If that fails, or if this is the first time the app is running,
// then use a random port.
//
// Returns the TCP listener and the address it's actually bound to.
async fn configure_tcp(
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
                    "Failed to get local address of {} TCP listener: {:?}",
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

async fn configure_quic(
    preferred_addr: SocketAddr,
    config: &ConfigStore,
) -> Option<(quic::Connector, quic::Acceptor, quic::SideChannel)> {
    let (proto, config_key) = match preferred_addr {
        SocketAddr::V4(_) => ("IPv4", config_keys::LAST_USED_UDP_PORT_V4_KEY),
        SocketAddr::V6(_) => ("IPv6", config_keys::LAST_USED_UDP_PORT_V6_KEY),
    };

    let socket = match socket::bind::<UdpSocket>(preferred_addr, config.entry(config_key)).await {
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
