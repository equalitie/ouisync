use crate::scoped_task::ScopedJoinHandle;
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
pub struct ReplicaDiscovery {
    listener_port: u16,
    socket: Arc<UdpSocket>,
    socket_addr: SocketAddr,
    _beacon_handle: ScopedJoinHandle<()>,
}

impl ReplicaDiscovery {
    /// Newly discovered replicas are reported on `tx` and their `RuntimeId` is placed into a
    /// LRU cache so as to not re-report it too frequently. Once the peer disconnects, the user of
    /// `ReplicaDiscovery` should call `forget` with the `RuntimeId` and the replica shall start
    /// reporting it again.
    pub fn new(listener_port: u16) -> io::Result<Self> {
        let socket = create_multicast_socket()?;
        let socket = Arc::new(socket);
        let socket_addr = socket.local_addr()?;

        let beacon_handle = task::spawn(run_beacon(socket.clone(), listener_port));
        let beacon_handle = ScopedJoinHandle(beacon_handle);

        Ok(Self {
            listener_port,
            socket,
            socket_addr,
            _beacon_handle: beacon_handle,
        })
    }

    pub async fn recv(&self) -> Option<SocketAddr> {
        let mut recv_buffer = [0; 64];

        let (message, addr) = loop {
            let (size, addr) = match self.socket.recv_from(&mut recv_buffer).await {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("Failed to receive discovery message: {}", error);
                    return None;
                }
            };

            // This is us, ignore.
            if addr == self.socket_addr {
                continue;
            }

            match bincode::deserialize(&recv_buffer[..size]) {
                Ok(message) => break (message, addr),
                Err(error) => {
                    log::error!("Malformed discovery message: {}", error);
                    continue;
                }
            }
        };

        let (is_rq, listener_port) = match message {
            Message::ImHereYouAll { port } => (true, port),
            Message::Reply { port } => (false, port),
        };

        if is_rq {
            // TODO: Consider `spawn`ing this, so it doesn't block this function.
            if let Err(error) = send(
                &self.socket,
                &Message::Reply {
                    port: self.listener_port,
                },
                addr,
            )
            .await
            {
                log::error!("Failed to send discovery message: {}", error);
            }
        }

        Some(SocketAddr::new(addr.ip(), listener_port))
    }
}

fn create_multicast_socket() -> io::Result<tokio::net::UdpSocket> {
    // Using net2 because, std::net, nor async_std::net nor tokio::net lets
    // one set reuse_address(true) before "binding" the socket.
    let sync_socket = net2::UdpBuilder::new_v4()?
        .reuse_address(true)?
        .bind((Ipv4Addr::UNSPECIFIED, MULTICAST_PORT))?;

    sync_socket.join_multicast_v4(&MULTICAST_ADDR, &Ipv4Addr::UNSPECIFIED)?;

    // This is not necessary if this is moved to async_std::net::UdpSocket,
    // but is if moved to tokio::net::UdpSocket.
    sync_socket.set_nonblocking(true)?;

    tokio::net::UdpSocket::from_std(sync_socket)
}

async fn run_beacon(socket: Arc<UdpSocket>, listener_port: u16) {
    let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

    loop {
        if let Err(error) = send(
            &socket,
            &Message::ImHereYouAll {
                port: listener_port,
            },
            multicast_endpoint,
        )
        .await
        {
            log::error!("Failed to send discovery message: {}", error);
            break;
        }

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
    ImHereYouAll { port: u16 },
    Reply { port: u16 },
}
