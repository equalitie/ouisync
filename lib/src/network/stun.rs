//! STUN protocol handling

use super::{peer_addr::PeerAddr, stun_server_list::STUN_SERVERS};
use futures_util::{future, StreamExt};
use net::{
    quic::SideChannel,
    stun::{NatBehavior, StunClient},
    udp::DatagramSocket,
};
use rand::seq::SliceRandom;
use std::{
    future::Future,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{select, sync::mpsc, time};
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

const LOOKUP_HOST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONCURRENCY: usize = 4;

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

    /// Determines the behavior of the NAT we are behind. Returns `None` if unknown.
    pub async fn nat_behavior(&self) -> Option<NatBehavior> {
        // Find IPv4 client
        let client = self
            .clients
            .lock()
            .unwrap()
            .iter()
            .find(|client| {
                client
                    .get_ref()
                    .local_addr()
                    .map(|local_addr| local_addr.is_ipv4())
                    .unwrap_or(false)
            })
            .cloned()?;

        nat_behavior(client).await
    }
}

async fn external_addr(client: Arc<StunClient<SideChannel>>) -> Option<PeerAddr> {
    let client = client.as_ref();
    let local_addr = client.get_ref().local_addr().ok()?;

    run(|server_addr| async move {
        if !is_same_family(&server_addr, &local_addr) {
            return None;
        }

        match client.external_addr(server_addr).await {
            Ok(addr) => {
                tracing::debug!("got external address: {addr}");
                // Currently this works for UDP (QUIC) only.
                Some(PeerAddr::Quic(addr))
            }
            Err(error) => {
                tracing::debug!("failed to get external address: {error:?}");
                None
            }
        }
    })
    .await
}

async fn nat_behavior(client: Arc<StunClient<SideChannel>>) -> Option<NatBehavior> {
    let client = client.as_ref();

    run(|server_addr| async move {
        match client.nat_mapping(server_addr).await {
            Ok(nat) => {
                tracing::debug!("got NAT behavior: {nat:?}");
                Some(nat)
            }
            Err(error) => {
                tracing::debug!("failed to get NAT behavior: {error:?}");
                None
            }
        }
    })
    .await
}

/// Runs task on every STUN server until one of them succeeds.
async fn run<F, Fut, R>(mut f: F) -> Option<R>
where
    F: FnMut(SocketAddr) -> Fut,
    Fut: Future<Output = Option<R>>,
{
    // Try all the servers in random order.
    let mut hosts: Vec<_> = STUN_SERVERS.to_vec();
    hosts.shuffle(&mut rand::thread_rng());

    let (tasks_tx, tasks_rx) = mpsc::channel(32);

    // Resolve the individual server hosts sequentially (to avoid getting rate-limitted) but run
    // the whole thing concurrently with the tasks. Run the tasks themselves also concurrently, but
    // with a max concurency.
    let push = async {
        for host in hosts {
            let span = tracing::info_span!("stun_server", message = host);

            let server_addrs =
                match time::timeout(LOOKUP_HOST_TIMEOUT, tokio::net::lookup_host(host)).await {
                    Ok(Ok(addrs)) => addrs,
                    Ok(Err(_)) | Err(_) => {
                        let _enter = span.enter();
                        tracing::debug!(stun_server = host, "failed to resolve host");
                        continue;
                    }
                };

            for server_addr in server_addrs {
                tasks_tx
                    .send(f(server_addr).instrument(span.clone()))
                    .await
                    .unwrap();
            }
        }

        future::pending::<()>().await;
    };

    let pull = async {
        ReceiverStream::new(tasks_rx)
            .buffer_unordered(MAX_CONCURRENCY)
            .filter_map(future::ready)
            .next()
            .await
    };

    select! {
        _ = push => unreachable!(),
        output = pull => output,
    }
}

fn is_same_family(a: &SocketAddr, b: &SocketAddr) -> bool {
    match (a, b) {
        (SocketAddr::V4(_), SocketAddr::V4(_)) | (SocketAddr::V6(_), SocketAddr::V6(_)) => true,
        (SocketAddr::V4(_), SocketAddr::V6(_)) | (SocketAddr::V6(_), SocketAddr::V4(_)) => false,
    }
}
