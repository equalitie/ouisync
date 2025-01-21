//!
//! Legacy local discovery code. It's a custom protocol that multicasts UDP packets over found
//! interfaces. It has two known issues:
//!
//! 1. The rate at which multicast packets are sent doesn't consider the amount of devices present
//!    on the network. Thus if there is too many devices, the network may get flooded.
//! 2. iOS devices require special permissions that Apple needs to grant to do multicasting.
//!
//! This code may be considered deprecated, but let's keep it for a while so that older ouisync
//! versions can still connect to the newer ones using mDNS.
//!
use crate::collections::HashMap;
use crate::network::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::{SeenPeer, SeenPeers},
};
use deadlock::AsyncMutex;
use futures_util::StreamExt;
use if_watch::{tokio::IfWatcher, IfEvent};
use net::udp::{DatagramSocket, UdpSocket, MULTICAST_ADDR, MULTICAST_PORT};
use rand::rngs::OsRng;
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use serde::{Deserialize, Serialize};
use state_monitor::StateMonitor;
use std::{
    future, io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    sync::mpsc,
    time::{sleep, Duration},
};
use tracing::{Instrument, Span};

// Time to wait when an error occurs on a socket.
const ERROR_DELAY: Duration = Duration::from_secs(3);

const PROTOCOL_MAGIC: &[u8; 17] = b"OUISYNC_DISCOVERY";
const PROTOCOL_VERSION: u8 = 0;

pub struct LocalDiscovery {
    peer_rx: mpsc::Receiver<SeenPeer>,
    _work_handle: ScopedJoinHandle<()>,
}

#[derive(Copy, Clone)]
pub enum Mode {
    // Full mode where we do multicast on a socket as well as receive signals from others.
    ObserveAndSignal,
    // The above is no longer used apart from the local-discovery utility, but for compatibility
    // with older ouisync version we'll keep observing on the multicast UDP socket (if the OS
    // permits us) to find those old peers as well.
    ObserveOnly,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort, monitor: Option<StateMonitor>, mode: Mode) -> Self {
        let (peer_tx, peer_rx) = mpsc::channel(1);

        let work_handle = scoped_task::spawn(
            async move {
                let mut inner = LocalDiscoveryInner {
                    mode,
                    listener_port,
                    peer_tx,
                    per_interface_discovery: HashMap::default(),
                };

                let mut interface_watcher = match IfWatcher::new() {
                    Ok(watch) => watch,
                    Err(error) => {
                        tracing::error!(?error, "failed to initialize network interface watcher");
                        return;
                    }
                };

                while let Some(event) = interface_watcher.next().await {
                    let event = match event {
                        Ok(event) => event,
                        Err(error) => {
                            tracing::error!(?error, "failed to poll network interface watcher");
                            break;
                        }
                    };

                    match event {
                        IfEvent::Up(addr) => inner.add(addr.addr(), monitor.as_ref()),
                        IfEvent::Down(addr) => inner.remove(addr.addr()),
                    }
                }
            }
            .instrument(Span::current()),
        );

        Self {
            peer_rx,
            _work_handle: work_handle,
        }
    }

    pub async fn recv(&mut self) -> SeenPeer {
        // NOTE: This *almost* never returns `None`. One exception is if `LocalDiscovery` is
        // created while the runtime is shutting down. Then it can happen that the worker task is
        // never started and `peer_tx` is immediatelly dropped.
        match self.peer_rx.recv().await {
            Some(peer) => peer,
            None => {
                // To keep the API simple, instead of propagating the `None` we wait forever.
                // However, this happens only during runtime shutdown so in practice we don't wait
                // at all.
                future::pending().await
            }
        }
    }
}

struct LocalDiscoveryInner {
    mode: Mode,
    listener_port: PeerPort,
    peer_tx: mpsc::Sender<SeenPeer>,
    per_interface_discovery: HashMap<Ipv4Addr, PerInterfaceLocalDiscovery>,
}

impl LocalDiscoveryInner {
    fn add(&mut self, interface: IpAddr, parent_monitor: Option<&StateMonitor>) {
        use std::collections::hash_map::Entry;

        if interface.is_loopback() {
            return;
        }

        let IpAddr::V4(interface) = interface else {
            return;
        };

        match self.per_interface_discovery.entry(interface) {
            Entry::Vacant(entry) => {
                let _enter = tracing::info_span!("local_discovery", %interface).entered();
                let discovery = PerInterfaceLocalDiscovery::new(
                    self.mode,
                    self.peer_tx.clone(),
                    self.listener_port,
                    interface,
                    parent_monitor,
                );

                match discovery {
                    Ok(discovery) => {
                        entry.insert(discovery);
                        tracing::info!("Local discovery started");
                    }
                    Err(error) => {
                        tracing::warn!(?error, "Failed to start local discovery");
                    }
                }
            }
            Entry::Occupied(_) => unreachable!(),
        }
    }

    fn remove(&mut self, interface: IpAddr) {
        let IpAddr::V4(interface) = interface else {
            return;
        };

        self.per_interface_discovery.remove(&interface);
    }
}

struct PerInterfaceLocalDiscovery {
    _beacon_handle: ScopedJoinHandle<()>,
    _receiver_handle: ScopedJoinHandle<()>,
    span: Span,
}

impl PerInterfaceLocalDiscovery {
    pub fn new(
        mode: Mode,
        peer_tx: mpsc::Sender<SeenPeer>,
        listener_port: PeerPort,
        interface: Ipv4Addr,
        parent_monitor: Option<&StateMonitor>,
    ) -> io::Result<Self> {
        // Only used to filter out multicast packets from self.
        let id = OsRng.gen();
        let socket_provider = Arc::new(SocketProvider::new(interface));

        let monitor = parent_monitor.map(|m| m.make_child(format!("{interface}")));
        let span = Span::current();

        let seen_peers = SeenPeers::new();

        let beacon_handle = scoped_task::spawn(
            run_beacon(
                mode,
                socket_provider.clone(),
                id,
                listener_port,
                seen_peers.clone(),
                monitor.clone(),
            )
            .instrument(span.clone()),
        );

        let receiver_handle = scoped_task::spawn(
            Self::run_recv_loop(
                peer_tx,
                id,
                listener_port,
                socket_provider,
                seen_peers,
                monitor,
            )
            .instrument(span.clone()),
        );

        Ok(Self {
            _beacon_handle: beacon_handle,
            _receiver_handle: receiver_handle,
            span,
        })
    }

    async fn run_recv_loop(
        peer_tx: mpsc::Sender<SeenPeer>,
        self_id: InsecureRuntimeId,
        listener_port: PeerPort,
        socket_provider: Arc<SocketProvider>,
        seen_peers: SeenPeers,
        monitor: Option<StateMonitor>,
    ) {
        let mut recv_buffer = [0; 64];
        let mut recv_error_reported = false;

        let beacon_requests_received = monitor
            .as_ref()
            .map(|m| m.make_value("beacon requests received", 0));
        let beacon_responses_received = monitor
            .as_ref()
            .map(|m| m.make_value("beacon responses received", 0));

        loop {
            let socket = socket_provider.provide().await;

            let (size, addr) = match socket.recv_from(&mut recv_buffer).await {
                Ok(pair) => {
                    recv_error_reported = false;
                    pair
                }
                Err(error) => {
                    if !recv_error_reported {
                        recv_error_reported = true;
                        tracing::error!("Failed to receive discovery message: {}", error);
                    }
                    socket_provider.mark_bad(socket).await;
                    sleep(ERROR_DELAY).await;
                    continue;
                }
            };

            let versioned_message: VersionedMessage =
                match bincode::deserialize(&recv_buffer[..size]) {
                    Ok(versioned_message) => versioned_message,
                    Err(error) => {
                        tracing::error!("Malformed discovery message: {}", error);
                        continue;
                    }
                };

            if &versioned_message.magic != PROTOCOL_MAGIC
                || versioned_message.version != PROTOCOL_VERSION
            {
                tracing::warn!(
                    "Incompatible protocol version (our:{}, their:{})",
                    PROTOCOL_VERSION,
                    versioned_message.version
                );
                continue;
            }

            let (socket, port, is_request, addr) = match versioned_message.message {
                Message::ImHereYouAll { id, .. } | Message::Reply { id, .. } if id == self_id => {
                    continue
                }
                Message::ImHereYouAll { port, .. } => (socket, port, true, addr),
                Message::Reply { port, .. } => (socket, port, false, addr),
            };

            if is_request {
                if let Some(value) = &beacon_requests_received {
                    *value.get() += 1;
                }

                let msg = Message::Reply {
                    port: listener_port,
                    id: self_id,
                };

                // TODO: Consider `spawn`ing this, so it doesn't block this function.
                if let Err(error) = send(&socket, msg, addr).await {
                    tracing::error!("Failed to send discovery message: {}", error);
                    socket_provider.mark_bad(socket).await;
                }
            } else {
                if let Some(value) = &beacon_responses_received {
                    *value.get() += 1;
                }
            }

            let addr = match port {
                PeerPort::Tcp(port) => PeerAddr::Tcp(SocketAddr::new(addr.ip(), port)),
                PeerPort::Quic(port) => PeerAddr::Quic(SocketAddr::new(addr.ip(), port)),
            };

            if let Some(peer) = seen_peers.insert(addr) {
                if peer_tx.send(peer).await.is_err() {
                    // The interface watcher removed the interface corresponding to this discovery
                    // instance.
                    break;
                }
            }
        }
    }
}

impl Drop for PerInterfaceLocalDiscovery {
    fn drop(&mut self) {
        let _enter = self.span.enter();
        tracing::info!("Local discovery stopped");
    }
}

async fn run_beacon(
    mode: Mode,
    socket_provider: Arc<SocketProvider>,
    id: InsecureRuntimeId,
    listener_port: PeerPort,
    seen_peers: SeenPeers,
    monitor: Option<StateMonitor>,
) {
    let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

    let beacons_sent = monitor.as_ref().map(|m| m.make_value("beacons sent", 0));
    let mut error_shown = false;

    loop {
        let socket = socket_provider.provide().await;

        seen_peers.start_new_round();

        let max_delay_secs = 8;

        let delay = match mode {
            Mode::ObserveAndSignal => {
                let msg = Message::ImHereYouAll {
                    id,
                    port: listener_port,
                };

                match send(&socket, msg, multicast_endpoint).await {
                    Ok(()) => {
                        error_shown = false;
                        if let Some(value) = &beacons_sent {
                            *value.get() += 1;
                        }
                    }
                    Err(error) => {
                        if !error_shown {
                            error_shown = true;
                            tracing::error!("Failed to send discovery message: {}", error);
                        }
                        socket_provider.mark_bad(socket).await;
                        sleep(ERROR_DELAY).await;
                        continue;
                    }
                }

                rand::thread_rng().gen_range(2..max_delay_secs)
            }
            // If we're not doing any signalling, we still want to count rounds to remove peers if
            // they haven't been seen for a while.
            Mode::ObserveOnly => max_delay_secs,
        };

        sleep(Duration::from_secs(delay)).await;
    }
}

async fn send(socket: &UdpSocket, message: Message, addr: SocketAddr) -> io::Result<()> {
    let data = bincode::serialize(&VersionedMessage {
        magic: *PROTOCOL_MAGIC,
        version: PROTOCOL_VERSION,
        message,
    })
    .unwrap();
    socket.send_to(&data, addr).await?;
    Ok(())
}

type InsecureRuntimeId = [u8; 16];

#[derive(Serialize, Deserialize, Debug)]
struct VersionedMessage {
    magic: [u8; 17],
    version: u8,
    message: Message,
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    ImHereYouAll {
        id: InsecureRuntimeId,
        port: PeerPort,
    },
    Reply {
        id: InsecureRuntimeId,
        port: PeerPort,
    },
}

struct SocketProvider {
    interface: Ipv4Addr,
    socket: AsyncMutex<Option<Arc<UdpSocket>>>,
}

impl SocketProvider {
    fn new(interface: Ipv4Addr) -> Self {
        Self {
            interface,
            socket: AsyncMutex::new(None),
        }
    }

    async fn provide(&self) -> Arc<UdpSocket> {
        let mut guard = self.socket.lock().await;

        match &*guard {
            Some(socket) => socket.clone(),
            None => {
                let mut last_error: Option<io::ErrorKind> = None;

                let socket = loop {
                    match UdpSocket::bind_multicast(self.interface) {
                        Ok(socket) => break Arc::new(socket),
                        Err(error) => {
                            if last_error != Some(error.kind()) {
                                tracing::warn!("Failed to bind to multicast socket: {error:?}");
                                last_error = Some(error.kind());
                            }
                            sleep(ERROR_DELAY).await;
                        }
                    }
                };

                *guard = Some(socket.clone());
                socket
            }
        }
    }

    async fn mark_bad(&self, bad_socket: Arc<UdpSocket>) {
        let mut guard = self.socket.lock().await;

        if let Some(stored_socket) = &*guard {
            if Arc::ptr_eq(stored_socket, &bad_socket) {
                *guard = None;
            }
        }
    }
}
