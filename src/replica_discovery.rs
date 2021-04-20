use std::{io,
    sync::Arc,
    net::{ Ipv4Addr, SocketAddr },
    time::Duration,
    collections::HashSet
};
use serde::{Serialize, Deserialize};
use rand::{Rng};
use futures::future::{abortable, AbortHandle};
use tokio::sync::{Notify, Mutex};
use tokio::task::{spawn};
use tokio::time::{sleep};
use lru::LruCache;

// Poor man's local discovery using UDP multicast.
// XXX: We should probably use mDNS, but so far all libraries I tried had some issues.
pub struct ReplicaDiscovery {
    state: Arc<State>,
    notify: Arc<Notify>,
    beacon_abort: AbortHandle,
    receiver_abort: AbortHandle,
}

impl ReplicaDiscovery {
    pub fn new(listener_port: u16) -> io::Result<Self> {
        let notify = Arc::new(Notify::new());

        let state = State::new(listener_port, notify.clone())?;

        let s = Arc::new(state);
        let s1 = s.clone();
        let s2 = s.clone();

        let (beacon_fut, beacon_handle) = abortable(async move {
            s1.run_beacon().await.unwrap();
        });

        spawn(beacon_fut);

        let (recv_fut, recv_handle) = abortable(async move {
            s2.run_receiver().await.unwrap();
        });

        spawn(recv_fut);

        Ok(ReplicaDiscovery {
            state: s,
            notify: notify,
            beacon_abort: beacon_handle,
            receiver_abort: recv_handle,
        })
    }

    pub async fn wait_for_activity(&mut self) -> HashSet<SocketAddr> {
        loop {
            self.notify.notified().await;

            let mut found = self.state.found_replicas.lock().await;

            if found.is_empty() {
                continue;
            }

            let ret = found.clone();
            *found = HashSet::new();
            return ret;
        }
    }
}

impl Drop for ReplicaDiscovery {
    fn drop(&mut self) {
        self.beacon_abort.abort();
        self.receiver_abort.abort();
    }
}

const ID_LEN: usize = 16; // 128 bits
type Id = [u8; ID_LEN];

struct State {
    id: Id,
    listener_port: u16,
    socket: tokio::net::UdpSocket,
    found_replicas: Mutex<HashSet<SocketAddr>>,
    notify: Arc<Notify>,
}

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;

const ADDR_ANY: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    ImHereYouAll { id: Id, port: u16 },
    Reply { id: Id, port: u16 },
}

impl State {
    fn new(listener_port: u16, notify: Arc<Notify>) -> io::Result<Self> {
        // Using net2 because, std::net, nor async_std::net nor tokio::net lets
        // one set reuse_address(true) before "binding" the socket.
        let sync_socket = net2::UdpBuilder::new_v4()?
            .reuse_address(true)?
            .bind((ADDR_ANY, MULTICAST_PORT))?;

        sync_socket.join_multicast_v4(&MULTICAST_ADDR, &ADDR_ANY)?;

        // This is not necessary if this is moved to async_std::net::UdpSocket,
        // but is if moved to tokio::net::UdpSocket.
        sync_socket.set_nonblocking(true)?;

        Ok(Self {
            id: rand::random(),
            listener_port,
            socket: tokio::net::UdpSocket::from_std(sync_socket).unwrap(),
            found_replicas: Mutex::new(HashSet::new()),
            notify,
        })
    }

    async fn run_beacon(&self) -> io::Result<()> {
        let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

        loop {
            self.send(&self.query(), multicast_endpoint).await?;
            let delay = rand::thread_rng().gen_range(2..8);
            sleep(Duration::from_secs(delay)).await;
        }
    }

    async fn run_receiver(&self) -> io::Result<()> {
        let mut recv_buffer = vec![0; 4096];

        let mut seen = LruCache::new(256);

        loop {
            let (size, addr) = self.socket.recv_from(&mut recv_buffer).await?;

            let r: Message = match bincode::deserialize(&recv_buffer[..size]) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let (is_rq, id, listener_port) = match r {
                Message::ImHereYouAll{id, port} => (true,  id, port),
                Message::Reply{id, port}        => (false, id, port),
            };

            if id == self.id {
                continue;
            }

            if seen.get(&id).is_some() { continue; }

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

    async fn send(&self, message : &Message, addr : SocketAddr) -> io::Result<()> {
        let data = bincode::serialize(&message).unwrap();
        self.socket.send_to(&data, addr).await?;
        Ok(())
    }

    fn query(&self) -> Message {
        Message::ImHereYouAll{id: self.id, port: self.listener_port}
    }

    fn reply(&self) -> Message {
        Message::Reply{id: self.id, port: self.listener_port}
    }
}
