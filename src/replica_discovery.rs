use crate::async_object::{AbortHandles, AsyncObject, AsyncObjectTrait};
use lru::LruCache;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, Notify};
use tokio::time::sleep;

const ID_LEN: usize = 16; // 128 bits
type Id = [u8; ID_LEN];

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;

const ADDR_ANY: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub struct ReplicaDiscovery {
    id: Id,
    listener_port: u16,
    socket: tokio::net::UdpSocket,
    found_replicas: Mutex<HashSet<SocketAddr>>,
    notify: Arc<Notify>,
    abort_handles: AbortHandles,
}

impl ReplicaDiscovery {
    pub fn new(listener_port: u16) -> io::Result<AsyncObject<Self>> {
        let notify = Arc::new(Notify::new());

        let n = Arc::new(ReplicaDiscovery {
            id: rand::random(),
            listener_port,
            socket: Self::create_multicast_socket()?,
            found_replicas: Mutex::new(HashSet::new()),
            notify: notify.clone(),
            abort_handles: AbortHandles::new(),
        });

        let n1 = n.clone();
        let n2 = n.clone();

        n.abortable_spawn(async move {
            n1.run_beacon().await.unwrap();
        });

        n.abortable_spawn(async move {
            n2.run_receiver().await.unwrap();
        });

        Ok(AsyncObject::new(n))
    }

    pub async fn wait_for_activity(&self) -> HashSet<SocketAddr> {
        loop {
            self.notify.notified().await;

            let mut found = self.found_replicas.lock().await;

            if found.is_empty() {
                continue;
            }

            let ret = found.clone();
            found.clear();

            return ret;
        }
    }

    fn create_multicast_socket() -> io::Result<tokio::net::UdpSocket> {
        // Using net2 because, std::net, nor async_std::net nor tokio::net lets
        // one set reuse_address(true) before "binding" the socket.
        let sync_socket = net2::UdpBuilder::new_v4()?
            .reuse_address(true)?
            .bind((ADDR_ANY, MULTICAST_PORT))?;

        sync_socket.join_multicast_v4(&MULTICAST_ADDR, &ADDR_ANY)?;

        // This is not necessary if this is moved to async_std::net::UdpSocket,
        // but is if moved to tokio::net::UdpSocket.
        sync_socket.set_nonblocking(true)?;

        tokio::net::UdpSocket::from_std(sync_socket)
    }

    async fn run_beacon(&self) -> io::Result<()> {
        let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

        loop {
            self.send(&self.query(), multicast_endpoint).await?;
            let delay = rand::thread_rng().gen_range(2..8);
            sleep(Duration::from_secs(delay)).await;
        }
    }

    async fn run_receiver(self: Arc<Self>) -> io::Result<()> {
        let mut recv_buffer = vec![0; 4096];

        let mut seen = LruCache::new(256);

        loop {
            let (size, addr) = self.socket.recv_from(&mut recv_buffer).await?;

            let r: Message = match bincode::deserialize(&recv_buffer[..size]) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let (is_rq, id, listener_port) = match r {
                Message::ImHereYouAll { id, port } => (true, id, port),
                Message::Reply { id, port } => (false, id, port),
            };

            if id == self.id {
                continue;
            }

            if seen.get(&id).is_some() {
                continue;
            }

            if is_rq {
                self.send(&self.reply(), addr).await?;
            }

            //println!("{:?}", listener_port);

            seen.put(id, ());

            let replica_addr = SocketAddr::new(addr.ip(), listener_port);
            self.found_replicas.lock().await.insert(replica_addr);

            self.notify.notify_one();
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
    ImHereYouAll { id: Id, port: u16 },
    Reply { id: Id, port: u16 },
}

impl AsyncObjectTrait for ReplicaDiscovery {
    fn abort_handles(&self) -> &AbortHandles {
        &self.abort_handles
    }
}
