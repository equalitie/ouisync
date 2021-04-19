use std::{io, sync::Arc, net::Ipv4Addr, time::Duration, net::SocketAddr, collections::HashSet};
use async_std::{sync::Mutex};
use async_std::net as async_net;
use serde::{Serialize, Deserialize};
use rand::{Rng};

const ID_LEN: usize = 16; // 128 bits
type Id = [u8; ID_LEN];

// Poor man's local discovery using UDP multicast. XXX: We should probably use mDNS, but so far all
// libraries I tried had some issues.
pub struct ReplicaDiscovery {
    id: Id,
    listener_addr: SocketAddr,
    socket: async_net::UdpSocket,
    send_mutex: Mutex<()>,
    found_replicas : Mutex<HashSet<SocketAddr>>,
}

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
const MULTICAST_PORT: u16 = 9271;

const ADDR_ANY: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    ImHereYouAll { id: Id, addr: SocketAddr },
    Reply { id: Id, addr: SocketAddr },
}

impl ReplicaDiscovery {
    pub fn new(listener_addr: SocketAddr) -> io::Result<Self> {
        let sync_socket = net2::UdpBuilder::new_v4()?
            .reuse_address(true)?
            .bind((ADDR_ANY, MULTICAST_PORT))?;

        sync_socket.join_multicast_v4(&MULTICAST_ADDR, &ADDR_ANY)?;

        Ok(Self{
            id: rand::random(),
            listener_addr: listener_addr,
            socket: async_net::UdpSocket::from(sync_socket),
            send_mutex: Mutex::new(()),
            found_replicas: Mutex::new(HashSet::new()),
        })
    }

    pub async fn run(self) -> Result<(), io::Error> {
        let this = Arc::new(self);
        let that = this.clone();

        let (beacon_fut, beacon_handle) = futures::future::abortable(async move {
            that.beacon().await.unwrap();
        });

        async_std::task::spawn(beacon_fut);

        this.receiver().await?;
        beacon_handle.abort();
        Ok(())
    }

    async fn beacon(&self) -> io::Result<()> {
        let multicast_endpoint = SocketAddr::new(MULTICAST_ADDR.into(), MULTICAST_PORT);

        loop {
            self.send(&self.query(), multicast_endpoint).await?;
            let delay = rand::thread_rng().gen_range(2..8);
            async_std::task::sleep(Duration::from_secs(delay)).await;
        }
    }

    async fn receiver(&self) -> io::Result<()> {
        let mut recv_buffer = vec![0; 4096];

        loop {
            let (size, addr) = self.socket.recv_from(&mut recv_buffer).await?;

            let r : Message = match bincode::deserialize(&recv_buffer[..size]) {
                Ok(r) => r,
                Err(_) => continue
            };

            let (is_rq, id, listener_addr) = match r {
                Message::ImHereYouAll{id, addr} => (true,  id, addr),
                Message::Reply{id, addr}        => (false, id, addr),
            };

            if id == self.id { continue; }

            if is_rq {
                self.send(&self.reply(), addr).await?;
            }

            println!("{:?}", listener_addr);

            self.found_replicas.lock().await.insert(listener_addr);
        }
    }

    async fn send(&self, message : &Message, addr : SocketAddr) -> io::Result<()> {
        let _guard = self.send_mutex.lock().await;
        let data = bincode::serialize(&message).unwrap();
        self.socket.send_to(&data, addr).await?;
        Ok(())
    }

    fn query(&self) -> Message {
        Message::ImHereYouAll{id: self.id, addr: self.listener_addr}
    }

    fn reply(&self) -> Message {
        Message::Reply{id: self.id, addr: self.listener_addr}
    }
}
