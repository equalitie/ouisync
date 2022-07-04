use super::{
    peer_addr::{PeerAddr, PeerPort},
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{
    scoped_task::ScopedJoinHandle,
    state_monitor::{MonitoredValue, StateMonitor},
};
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{net::UdpSocket, sync::Mutex, task, time::sleep};

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;
// Time to wait when an error occurs on a socket.
const ERROR_DELAY: Duration = Duration::from_secs(3);

const PROTOCOL_MAGIC: &[u8; 17] = b"OUISYNC_DISCOVERY";
const PROTOCOL_VERSION: u8 = 0;

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub(super) struct LocalDiscovery {
    // Only used to filter out multicast packets from self.
    id: InsecureRuntimeId,
    listener_port: PeerPort,
    socket_provider: Arc<SocketProvider>,
    // TODO: SeenPeers implements Clone, so doesn't need to be in Arc.
    seen_peers: Arc<SeenPeers>,
    beacon_requests_received: MonitoredValue<u64>,
    beacon_responses_received: MonitoredValue<u64>,
    _beacon_handle: ScopedJoinHandle<()>,
}

impl LocalDiscovery {
    pub fn new(listener_port: PeerPort, monitor: Arc<StateMonitor>) -> io::Result<Self> {
        let id = OsRng.gen();
        let socket_provider = Arc::new(SocketProvider::new());

        let beacon_requests_received = monitor.make_value("beacon_requests_received".into(), 0);
        let beacon_responses_received = monitor.make_value("beacon_responses_received".into(), 0);

        let seen_peers = Arc::new(SeenPeers::new());

        let beacon_handle = task::spawn(run_beacon(
            socket_provider.clone(),
            id,
            listener_port,
            seen_peers.clone(),
            monitor,
        ));
        let beacon_handle = ScopedJoinHandle(beacon_handle);

        Ok(Self {
            id,
            listener_port,
            socket_provider,
            seen_peers,
            beacon_requests_received,
            beacon_responses_received,
            _beacon_handle: beacon_handle,
        })
    }

    pub async fn recv(&self) -> Option<SeenPeer> {
        loop {
            let addr = match self.try_recv().await {
                Some(addr) => addr,
                None => return None,
            };

            if let Some(peer) = self.seen_peers.insert(addr) {
                return Some(peer);
            }
        }
    }

    async fn try_recv(&self) -> Option<PeerAddr> {
        let mut recv_buffer = [0; 64];

        let mut recv_error_reported = false;

        let (socket, port, is_request, addr) = loop {
            let socket = self.socket_provider.provide().await;

            let (size, addr) = match socket.recv_from(&mut recv_buffer).await {
                Ok(pair) => {
                    recv_error_reported = false;
                    pair
                }
                Err(error) => {
                    if !recv_error_reported {
                        recv_error_reported = true;
                        log::error!("Failed to receive discovery message: {}", error);
                    }
                    self.socket_provider.mark_bad(socket).await;
                    sleep(ERROR_DELAY).await;
                    continue;
                }
            };

            let versioned_message: VersionedMessage =
                match bincode::deserialize(&recv_buffer[..size]) {
                    Ok(versioned_message) => versioned_message,
                    Err(error) => {
                        log::error!("Malformed discovery message: {}", error);
                        continue;
                    }
                };

            if &versioned_message.magic != PROTOCOL_MAGIC
                || versioned_message.version != PROTOCOL_VERSION
            {
                log::warn!(
                    "Incompatible protocol version (our:{}, their:{})",
                    PROTOCOL_VERSION,
                    versioned_message.version
                );
                continue;
            }

            match versioned_message.message {
                Message::ImHereYouAll { id, .. } | Message::Reply { id, .. } if id == self.id => {
                    continue
                }
                Message::ImHereYouAll { port, .. } => break (socket, port, true, addr),
                Message::Reply { port, .. } => break (socket, port, false, addr),
            }
        };

        if is_request {
            *self.beacon_requests_received.get() += 1;

            let msg = Message::Reply {
                port: self.listener_port,
                id: self.id,
            };

            // TODO: Consider `spawn`ing this, so it doesn't block this function.
            if let Err(error) = send(&socket, msg, addr).await {
                log::error!("Failed to send discovery message: {}", error);
                self.socket_provider.mark_bad(socket).await;
            }
        } else {
            *self.beacon_responses_received.get() += 1;
        }

        let addr = match port {
            PeerPort::Tcp(port) => PeerAddr::Tcp(SocketAddr::new(addr.ip(), port)),
            PeerPort::Quic(port) => PeerAddr::Quic(SocketAddr::new(addr.ip(), port)),
        };

        Some(addr)
    }
}

fn create_multicast_socket() -> io::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Socket, Type};
    use std::net::SocketAddrV4;

    // Using socket2 because, std::net, nor async_std::net nor tokio::net lets
    // one set reuse_address(true) before "binding" the socket.
    let sync_socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    sync_socket.set_reuse_address(true)?;

    // FIXME: this might be blocking. We should make this whole function async and use
    // `block_in_place`.
    sync_socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MULTICAST_PORT).into())?;

    let sync_socket: std::net::UdpSocket = sync_socket.into();

    sync_socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;

    // This is not necessary if this is moved to async_std::net::UdpSocket,
    // but is if moved to tokio::net::UdpSocket.
    sync_socket.set_nonblocking(true)?;

    tokio::net::UdpSocket::from_std(sync_socket)
}

async fn run_beacon(
    socket_provider: Arc<SocketProvider>,
    id: InsecureRuntimeId,
    listener_port: PeerPort,
    seen_peers: Arc<SeenPeers>,
    monitor: Arc<StateMonitor>,
) {
    let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);
    let beacons_sent = monitor.make_value::<u64>("beacons_sent".into(), 0);

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
                *beacons_sent.get() += 1;
            }
            Err(error) => {
                if !error_shown {
                    error_shown = true;
                    log::error!("Failed to send discovery message: {}", error);
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
    socket: Mutex<Option<Arc<UdpSocket>>>,
}

impl SocketProvider {
    fn new() -> Self {
        Self {
            socket: Mutex::new(None),
        }
    }

    async fn provide(&self) -> Arc<UdpSocket> {
        let mut guard = self.socket.lock().await;

        match &*guard {
            Some(socket) => socket.clone(),
            None => {
                let socket = loop {
                    match create_multicast_socket() {
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
