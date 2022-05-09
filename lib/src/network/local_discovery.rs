use super::protocol::RuntimeId;
use crate::{
    scoped_task::ScopedJoinHandle,
    state_monitor::{MonitoredValue, StateMonitor},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{net::UdpSocket, task, time::sleep};

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub(super) struct LocalDiscovery {
    id: RuntimeId,
    listener_port: u16,
    socket: Arc<UdpSocket>,
    beacon_requests_received: MonitoredValue<u64>,
    beacon_responses_received: MonitoredValue<u64>,
    _beacon_handle: ScopedJoinHandle<()>,
}

impl LocalDiscovery {
    /// Newly discovered replicas are reported on `tx` and their `RuntimeId` is placed into a
    /// LRU cache so as to not re-report it too frequently. Once the peer disconnects, the user of
    /// `LocalDiscovery` should call `forget` with the `RuntimeId` and the replica shall start
    /// reporting it again.
    pub fn new(id: RuntimeId, listener_port: u16, monitor: Arc<StateMonitor>) -> io::Result<Self> {
        let socket = create_multicast_socket()?;
        let socket = Arc::new(socket);

        let beacon_requests_received = monitor.make_value("beacon-requests-received".into(), 0);
        let beacon_responses_received = monitor.make_value("beacon-responses-received".into(), 0);

        let beacon_handle = task::spawn(run_beacon(socket.clone(), id, listener_port, monitor));
        let beacon_handle = ScopedJoinHandle(beacon_handle);

        Ok(Self {
            id,
            listener_port,
            socket,
            beacon_requests_received,
            beacon_responses_received,
            _beacon_handle: beacon_handle,
        })
    }

    pub async fn recv(&self) -> Option<SocketAddr> {
        let mut recv_buffer = [0; 64];

        let (port, is_request, addr) = loop {
            let (size, addr) = match self.socket.recv_from(&mut recv_buffer).await {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("Failed to receive discovery message: {}", error);
                    return None;
                }
            };

            let message = match bincode::deserialize(&recv_buffer[..size]) {
                Ok(message) => message,
                Err(error) => {
                    log::error!("Malformed discovery message: {}", error);
                    continue;
                }
            };

            match message {
                Message::ImHereYouAll { id, .. } | Message::Reply { id, .. } if id == self.id => {
                    continue
                }
                Message::ImHereYouAll { port, .. } => break (port, true, addr),
                Message::Reply { port, .. } => break (port, false, addr),
            }
        };

        if is_request {
            *self.beacon_requests_received.get() += 1;

            // TODO: Consider `spawn`ing this, so it doesn't block this function.
            if let Err(error) = send(
                &self.socket,
                &Message::Reply {
                    port: self.listener_port,
                    id: self.id,
                },
                addr,
            )
            .await
            {
                log::error!("Failed to send discovery message: {}", error);
            }
        } else {
            *self.beacon_responses_received.get() += 1;
        }

        Some(SocketAddr::new(addr.ip(), port))
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
    socket: Arc<UdpSocket>,
    id: RuntimeId,
    listener_port: u16,
    monitor: Arc<StateMonitor>,
) {
    let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);
    let beacons_sent = monitor.make_value::<u64>("beacons-sent".into(), 0);

    loop {
        if let Err(error) = send(
            &socket,
            &Message::ImHereYouAll {
                id,
                port: listener_port,
            },
            multicast_endpoint,
        )
        .await
        {
            log::error!("Failed to send discovery message: {}", error);
            break;
        }

        *beacons_sent.get() += 1;

        let delay = rand::thread_rng().gen_range(2..8);
        sleep(Duration::from_secs(delay)).await;
    }
}

async fn send(socket: &UdpSocket, message: &Message, addr: SocketAddr) -> io::Result<()> {
    let data = bincode::serialize(message).unwrap();
    socket.send_to(&data, addr).await?;
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    ImHereYouAll { id: RuntimeId, port: u16 },
    Reply { id: RuntimeId, port: u16 },
}
