use crate::scoped_task::ScopedTaskSet;
use lru::LruCache;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc::Sender, Mutex};
use tokio::time::sleep;

/// ID of this replica runtime, it is different from the ReplicaId because it is generated randomly
/// on each ReplicaDiscovery instantiation.
const ID_LEN: usize = 16; // 128 bits
pub type RuntimeId = [u8; ID_LEN];

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub struct ReplicaDiscovery {
    inner: Arc<Inner>,
    _tasks: ScopedTaskSet,
}

impl ReplicaDiscovery {
    /// Newly discovered replicas are reported on `tx` and their `RuntimeId` is placed into a
    /// LRU cache so as to not re-report it too frequently. Once the peer disconnects, the user of
    /// `ReplicaDiscovery` should call `forget` with the `RuntimeId` and the replica shall start
    /// reporting it again.
    pub fn new(listener_port: u16, tx: Sender<(RuntimeId, SocketAddr)>) -> io::Result<Self> {
        let inner = Arc::new(Inner {
            id: rand::random(),
            listener_port,
            socket: Self::create_multicast_socket()?,
            seen: Mutex::new(LruCache::new(256)),
        });

        let tasks = ScopedTaskSet::default();

        tasks.spawn(inner.clone().run_beacon());
        tasks.spawn(inner.clone().run_receiver(tx));

        Ok(Self {
            inner,
            _tasks: tasks,
        })
    }

    ///
    /// Remove the id of the remote replica from the LRU cache so frequent announcment can start
    /// happening again.
    ///
    pub async fn forget(&self, id: &RuntimeId) {
        self.inner.seen.lock().await.pop(id);
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
}

struct Inner {
    id: RuntimeId,
    listener_port: u16,
    socket: tokio::net::UdpSocket,
    seen: Mutex<LruCache<RuntimeId, ()>>,
}

impl Inner {
    async fn run_beacon(self: Arc<Self>) {
        let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

        loop {
            if let Err(error) = self.send(&self.query(), multicast_endpoint).await {
                log::error!("Failed to send discovery message: {}", error);
                break;
            }

            let delay = rand::thread_rng().gen_range(2..8);
            sleep(Duration::from_secs(delay)).await;
        }
    }

    async fn run_receiver(self: Arc<Self>, tx: Sender<(RuntimeId, SocketAddr)>) {
        let mut recv_buffer = vec![0; 4096];

        loop {
            let (size, addr) = match self.socket.recv_from(&mut recv_buffer).await {
                Ok(pair) => pair,
                Err(error) => {
                    log::error!("Failed to receive discovery message: {}", error);
                    break;
                }
            };

            let r: Message = match bincode::deserialize(&recv_buffer[..size]) {
                Ok(r) => r,
                Err(error) => {
                    log::error!("Malformed discovery message: {}", error);
                    continue;
                }
            };

            let (is_rq, id, listener_port) = match r {
                Message::ImHereYouAll { id, port } => (true, id, port),
                Message::Reply { id, port } => (false, id, port),
            };

            if id == self.id {
                continue;
            }

            {
                let mut seen = self.seen.lock().await;

                if seen.get(&id).is_some() {
                    continue;
                }

                seen.put(id, ());
            }

            if is_rq {
                if let Err(error) = self.send(&self.reply(), addr).await {
                    log::error!("Failed to send discovery message: {}", error);
                    break;
                }
            }

            let replica_addr = SocketAddr::new(addr.ip(), listener_port);
            tx.send((id, replica_addr)).await.unwrap_or(());
        }
    }

    async fn send(&self, message: &Message, addr: SocketAddr) -> io::Result<()> {
        let data = bincode::serialize(&message).unwrap();
        self.socket.send_to(&data, addr).await?;
        Ok(())
    }

    fn query(&self) -> Message {
        Message::ImHereYouAll {
            id: self.id,
            port: self.listener_port,
        }
    }

    fn reply(&self) -> Message {
        Message::Reply {
            id: self.id,
            port: self.listener_port,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    ImHereYouAll { id: RuntimeId, port: u16 },
    Reply { id: RuntimeId, port: u16 },
}
