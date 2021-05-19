use crate::scoped_task_set::ScopedTaskSet;
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

/// ID of this replica runtime, it is different from the ReplicaId because it is generated randomly
/// on each ReplicaDiscovery instantiation.
const ID_LEN: usize = 16; // 128 bits
pub type RuntimeId = [u8; ID_LEN];

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;
const ADDR_ANY: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub struct ReplicaDiscovery {
    state: Arc<State>,
    _tasks: ScopedTaskSet,
}

struct State {
    id: RuntimeId,
    listener_port: u16,
    socket: tokio::net::UdpSocket,
    found_replicas: Mutex<HashSet<(RuntimeId, SocketAddr)>>,
    seen: std::sync::Mutex<LruCache<RuntimeId, ()>>,
    notify: Arc<Notify>,
}

impl ReplicaDiscovery {
    pub fn new(listener_port: u16) -> io::Result<Self> {
        let notify = Arc::new(Notify::new());

        let state = Arc::new(State {
            id: rand::random(),
            listener_port,
            socket: Self::create_multicast_socket()?,
            found_replicas: Mutex::new(HashSet::new()),
            seen: std::sync::Mutex::new(LruCache::new(256)),
            notify,
        });

        let state1 = state.clone();
        let state2 = state.clone();

        let tasks = ScopedTaskSet::default();

        tasks.spawn(async move {
            state1.run_beacon().await.unwrap();
        });

        tasks.spawn(async move {
            state2.run_receiver().await.unwrap();
        });

        Ok(Self {
            state,
            _tasks: tasks,
        })
    }

    ///
    /// Wait for replicas to be found. Once some are, they are returned and their RuntimeId is
    /// placed into a LRU cache so as to not re-report it too frequently. Once the peer
    /// disconnects, the user of ReplicaDiscovery should call forget with the RuntimeId and the
    /// replica shall start reporting it again.
    ///
    pub async fn wait_for_activity(&self) -> HashSet<(RuntimeId, SocketAddr)> {
        loop {
            self.state.notify.notified().await;

            let mut found = self.state.found_replicas.lock().await;

            if found.is_empty() {
                continue;
            }

            let ret = found.clone();
            found.clear();

            return ret;
        }
    }

    ///
    /// Remove the id of the remote replica from the LRU cache so frequent announcment can start
    /// happening again.
    ///
    pub fn forget(&self, id: &RuntimeId) {
        self.state.seen.lock().unwrap().pop(id);
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
}

impl State {
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

            {
                let mut seen = self.seen.lock().unwrap();

                if seen.get(&id).is_some() {
                    continue;
                }

                seen.put(id, ());
            }

            if is_rq {
                self.send(&self.reply(), addr).await?;
            }

            //println!("{:?}", listener_port);

            let replica_addr = SocketAddr::new(addr.ip(), listener_port);
            self.found_replicas.lock().await.insert((id, replica_addr));

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
    ImHereYouAll { id: RuntimeId, port: u16 },
    Reply { id: RuntimeId, port: u16 },
}
