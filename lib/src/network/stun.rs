//! STUN protocol handling

use super::{peer_addr::PeerAddr, stun_server_list::STUN_SERVERS};
use futures_util::future;
use net::{quic::SideChannel, stun::StunClient};
use rand::seq::SliceRandom;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time;

const HOST_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct StunClients {
    clients: Mutex<Vec<Arc<StunClient<SideChannel>>>>,
}

impl StunClients {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(Vec::new()),
        }
    }

    /// Binds the STUN clients to the given sockets.
    pub fn rebind(&self, sockets: impl IntoIterator<Item = SideChannel>) {
        let mut clients = self.clients.lock().unwrap();
        clients.clear();
        clients.extend(sockets.into_iter().map(StunClient::new).map(Arc::new));
    }

    /// Queries our external addresses.
    pub async fn external_addrs(&self) -> Vec<PeerAddr> {
        let tasks: Vec<_> = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .map(|client| external_addr(client.clone()))
            .collect();

        future::join_all(tasks)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

async fn external_addr(client: Arc<StunClient<SideChannel>>) -> Option<PeerAddr> {
    let local_addr = client.get_ref().local_addr().ok()?;

    // Try all the servers, one at a time, in random order, until one of them succeeds or all
    // fail.
    let mut hosts: Vec<_> = STUN_SERVERS.to_vec();
    hosts.shuffle(&mut rand::thread_rng());

    for host in hosts {
        let server_addrs =
            match time::timeout(HOST_LOOKUP_TIMEOUT, tokio::net::lookup_host(host)).await {
                Ok(Ok(addrs)) => addrs,
                Ok(Err(_)) | Err(_) => continue,
            };

        for server_addr in server_addrs {
            if !is_same_family(&server_addr, &local_addr) {
                continue;
            }

            match client.external_addr(server_addr).await {
                Ok(addr) => {
                    tracing::debug!(stun_server = host, "got external address: {addr}");
                    // Currently this works for UDP (QUIC) only.
                    return Some(PeerAddr::Quic(addr));
                }
                Err(error) => {
                    tracing::debug!(stun_server = host, ?error, "failed to get external address");
                }
            }
        }
    }

    None
}

fn is_same_family(a: &SocketAddr, b: &SocketAddr) -> bool {
    match (a, b) {
        (SocketAddr::V4(_), SocketAddr::V4(_)) | (SocketAddr::V6(_), SocketAddr::V6(_)) => true,
        (SocketAddr::V4(_), SocketAddr::V6(_)) | (SocketAddr::V6(_), SocketAddr::V4(_)) => false,
    }
}
