use super::{
    interface::{self, InterfaceChange},
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    collections::{HashMap, HashSet},
    scoped_task::{self, ScopedJoinHandle},
};
use net::udp::{UdpSocket, MULTICAST_ADDR, MULTICAST_PORT};
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::{
    sync::{mpsc, Mutex},
    time::{sleep, Duration},
};
use tracing::Instrument;

// Time to wait when an error occurs on a socket.
const ERROR_DELAY: Duration = Duration::from_secs(3);

const PROTOCOL_MAGIC: &[u8; 17] = b"OUISYNC_DISCOVERY";
const PROTOCOL_VERSION: u8 = 0;

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS or DNS-SD, but so far all libraries I tried had some issues.
// http://http://dns-sd.org/
// One advantage of the above ones compared to our own is that we would be using a standart port
// for it.

pub(crate) struct LocalDiscovery {
    peer_rx: mpsc::Receiver<SeenPeer>,
    _work_handle: ScopedJoinHandle<()>,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort) -> Self {
        let (peer_tx, peer_rx) = mpsc::channel(1);
        let span = tracing::info_span!("LocalDiscovery");

        let work_handle = scoped_task::spawn(
            async move {
                let mut inner = LocalDiscoveryInner {
                    listener_port,
                    peer_tx,
                    per_interface_discovery: HashMap::default(),
                };

                let mut interface_watcher = interface::watch_ipv4_multicast_interfaces();

                while let Some(change) = interface_watcher.recv().await {
                    match change {
                        InterfaceChange::Added(set) => inner.add(set),
                        InterfaceChange::Removed(set) => inner.remove(set),
                    }
                }
            }
            .instrument(span),
        );

        Self {
            peer_rx,
            _work_handle: work_handle,
        }
    }

    pub async fn recv(&mut self) -> SeenPeer {
        // Unwrap is OK because we don't expect the `LocalDiscoveryInner` instance to get destroyed
        // before this `LocalDiscovery` instance.
        self.peer_rx.recv().await.unwrap()
    }
}

struct LocalDiscoveryInner {
    listener_port: PeerPort,
    peer_tx: mpsc::Sender<SeenPeer>,
    per_interface_discovery: HashMap<Ipv4Addr, PerInterfaceLocalDiscovery>,
}

impl LocalDiscoveryInner {
    fn add(&mut self, new_interfaces: HashSet<Ipv4Addr>) {
        use crate::collections::hash_map::Entry;

        for interface in new_interfaces {
            match self.per_interface_discovery.entry(interface) {
                Entry::Vacant(entry) => {
                    let discovery = PerInterfaceLocalDiscovery::new(
                        self.peer_tx.clone(),
                        self.listener_port,
                        interface,
                    );

                    match discovery {
                        Ok(discovery) => {
                            entry.insert(discovery);
                            tracing::info!("Started local discovery on interface {:?}", interface);
                        }
                        Err(err) => {
                            tracing::warn!(
                                "Failed to start local discovery on interface {:?}: {:?}",
                                interface,
                                err
                            );
                        }
                    }
                }
                Entry::Occupied(_) => unreachable!(),
            }
        }
    }

    fn remove(&mut self, removed_interfaces: HashSet<Ipv4Addr>) {
        for interface in removed_interfaces {
            if self.per_interface_discovery.remove(&interface).is_some() {
                log_stop(interface);
            }
        }
    }
}

impl Drop for LocalDiscoveryInner {
    fn drop(&mut self) {
        for interface in self.per_interface_discovery.keys() {
            log_stop(*interface);
        }
    }
}

fn log_stop(addr: Ipv4Addr) {
    tracing::info!("Stopped local discovery on interface {:?}", addr);
}

struct PerInterfaceLocalDiscovery {
    _beacon_handle: ScopedJoinHandle<()>,
    _receiver_handle: ScopedJoinHandle<()>,
}

impl PerInterfaceLocalDiscovery {
    pub fn new(
        peer_tx: mpsc::Sender<SeenPeer>,
        listener_port: PeerPort,
        interface: Ipv4Addr,
    ) -> io::Result<Self> {
        // Only used to filter out multicast packets from self.
        let id = OsRng.gen();
        let socket_provider = Arc::new(SocketProvider::new(interface));

        let span = tracing::info_span!("interface", addr = ?interface);

        let seen_peers = SeenPeers::new();

        let beacon_handle = scoped_task::spawn(
            run_beacon(
                socket_provider.clone(),
                id,
                listener_port,
                seen_peers.clone(),
            )
            .instrument(span.clone()),
        );

        let receiver_handle = scoped_task::spawn(async move {
            Self::run_recv_loop(peer_tx, id, listener_port, socket_provider, seen_peers)
                .instrument(span)
                .await
        });

        Ok(Self {
            _beacon_handle: beacon_handle,
            _receiver_handle: receiver_handle,
        })
    }

    async fn run_recv_loop(
        peer_tx: mpsc::Sender<SeenPeer>,
        self_id: InsecureRuntimeId,
        listener_port: PeerPort,
        socket_provider: Arc<SocketProvider>,
        seen_peers: SeenPeers,
    ) {
        let mut recv_buffer = [0; 64];
        let mut recv_error_reported = false;

        let mut beacon_requests_received = 0;
        state_monitor!(beacon_requests_received);

        let mut beacon_responses_received = 0;
        state_monitor!(beacon_responses_received);

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
                beacon_requests_received += 1;
                state_monitor!(beacon_requests_received);

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
                beacon_responses_received += 1;
                state_monitor!(beacon_responses_received);
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

async fn run_beacon(
    socket_provider: Arc<SocketProvider>,
    id: InsecureRuntimeId,
    listener_port: PeerPort,
    seen_peers: SeenPeers,
) {
    let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

    let mut beacons_sent = 0;
    state_monitor!(beacons_sent);

    let mut error_shown = false;

    loop {
        let socket = socket_provider.provide().await;

        seen_peers.start_new_round();

        let msg = Message::ImHereYouAll {
            id,
            port: listener_port,
        };

        match send(&socket, msg, multicast_endpoint).await {
            Ok(()) => {
                error_shown = false;
                beacons_sent += 1;
                state_monitor!(beacons_sent);
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

        let delay = rand::thread_rng().gen_range(2..8);
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
    socket: Mutex<Option<Arc<UdpSocket>>>,
}

impl SocketProvider {
    fn new(interface: Ipv4Addr) -> Self {
        Self {
            interface,
            socket: Mutex::new(None),
        }
    }

    async fn provide(&self) -> Arc<UdpSocket> {
        let mut guard = self.socket.lock().await;

        match &*guard {
            Some(socket) => socket.clone(),
            None => {
                let socket = loop {
                    match UdpSocket::bind_multicast(self.interface).await {
                        Ok(socket) => break Arc::new(socket),
                        Err(_) => sleep(ERROR_DELAY).await,
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
