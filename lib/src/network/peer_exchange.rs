//! Peer exchange - a mechanism by which peers exchange information about other peers with each
//! other in order to discover new peers.

use super::{
    ip,
    message::Content,
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
};
use crate::{collections::HashSet, sync::AwaitDrop};
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    ops::Range,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{sync::mpsc, task, time};

/// How often we send contacts to other peers. It's a range that the actual interval is randomly
/// picked from so that the sends don't happen all at the same time.
const SEND_INTERVAL_RANGE: Range<Duration> = Duration::from_secs(60)..Duration::from_secs(2 * 60);

/// Duration of a single round for the `SeenPeers` machinery.
const ROUND_DURATION: Duration = Duration::from_secs(10 * 60);

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct PexPayload(HashSet<PeerAddr>);

/// Entry point to the peer exchange.
pub(crate) struct PexDiscovery {
    state: Arc<RwLock<State>>,
    seen_peers: SeenPeers,
    discover_tx: mpsc::Sender<SeenPeer>,
    _round_task: ScopedJoinHandle<()>,
}

impl PexDiscovery {
    pub fn new(discover_tx: mpsc::Sender<SeenPeer>) -> Self {
        let seen_peers = SeenPeers::new();

        let round_task = scoped_task::spawn({
            let seen_peers = seen_peers.clone();

            async move {
                loop {
                    time::sleep(ROUND_DURATION).await;
                    seen_peers.start_new_round();
                }
            }
        });

        Self {
            state: Arc::new(RwLock::new(State::default())),
            seen_peers,
            discover_tx,
            _round_task: round_task,
        }
    }

    pub fn new_peer(&self) -> PexPeer {
        let mut state = self.state.write().unwrap();
        let peer_id = state.peers.insert(PeerState::default());

        PexPeer {
            peer_id,
            state: self.state.clone(),
            seen_peers: self.seen_peers.clone(),
            discover_tx: self.discover_tx.clone(),
        }
    }

    pub fn new_repository(&self) -> PexRepository {
        let mut state = self.state.write().unwrap();
        let repo_id = state.repos.insert(RepoState::default());

        PexRepository {
            repo_id,
            state: self.state.clone(),
        }
    }
}

/// Handle to manage the peer exchange for a single repository.
#[derive(Clone)]
pub(crate) struct PexRepository {
    repo_id: RepoId,
    state: Arc<RwLock<State>>,
}

impl PexRepository {
    pub fn is_enabled(&self) -> bool {
        self.state.read().unwrap().repos[self.repo_id].enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.state.write().unwrap().repos[self.repo_id].enabled = enabled;
    }
}

impl Drop for PexRepository {
    fn drop(&mut self) {
        self.state.write().unwrap().repos.remove(self.repo_id);
    }
}

/// Handle to manage peer exchange for a single peer.
pub(crate) struct PexPeer {
    peer_id: PeerId,
    state: Arc<RwLock<State>>,
    seen_peers: SeenPeers,
    discover_tx: mpsc::Sender<SeenPeer>,
}

impl PexPeer {
    pub fn handle_connection(&self, addr: PeerAddr, closed: AwaitDrop) {
        if !self.state.write().unwrap().peers[self.peer_id]
            .addrs
            .insert(addr)
        {
            // Already added
            return;
        }

        // Spawn a task that removes the address when the connection gets closed.
        task::spawn({
            let peer_id = self.peer_id;
            let state = self.state.clone();

            async move {
                closed.await;

                if let Some(peer) = state.write().unwrap().peers.get_mut(peer_id) {
                    peer.addrs.remove(&addr);
                }
            }
        });
    }

    /// Create a pair of (sender, receiver) to manage peer exchange for a single link.
    pub fn new_link(&self, repo: &PexRepository) -> (PexSender, PexReceiver) {
        let tx = PexSender {
            repo_id: repo.repo_id,
            peer_id: self.peer_id,
            state: self.state.clone(),
        };

        let rx = PexReceiver {
            repo_id: repo.repo_id,
            state: self.state.clone(),
            seen_peers: self.seen_peers.clone(),
            discover_tx: self.discover_tx.clone(),
        };

        (tx, rx)
    }
}

impl Drop for PexPeer {
    fn drop(&mut self) {
        self.state.write().unwrap().peers.remove(self.peer_id);
    }
}

pub(crate) struct PexSender {
    repo_id: RepoId,
    peer_id: PeerId,
    state: Arc<RwLock<State>>,
}

impl PexSender {
    /// While this method is running, it periodically sends contacts of other peers that share the
    /// same repo to this peer and makes the contacts of this peer aailable to them.
    pub async fn run(&mut self, content_tx: mpsc::Sender<Content>) {
        let Some(collector) = self.enable() else {
            // Another collector for this link already exists.
            return;
        };

        loop {
            let Some(addrs) = collector.collect() else {
                break;
            };

            if !addrs.is_empty() {
                content_tx.send(Content::Pex(PexPayload(addrs))).await.ok();
            }

            let interval = rand::thread_rng().gen_range(SEND_INTERVAL_RANGE);
            time::sleep(interval).await;
        }
    }

    /// Makes the contacts of this peers available to other peers that share tha same repo as long
    /// as the returned `PexCollector` is in scope. Returns `None` if a collector for this link
    /// already exists.
    fn enable(&self) -> Option<PexCollector<'_>> {
        let mut state = self.state.write().unwrap();
        let repo = state.repos.get_mut(self.repo_id)?;

        if repo.peers.insert(self.peer_id) {
            Some(PexCollector(self))
        } else {
            None
        }
    }
}

pub(crate) struct PexCollector<'a>(&'a PexSender);

impl<'a> PexCollector<'a> {
    fn collect(&self) -> Option<HashSet<PeerAddr>> {
        let state = self.0.state.read().unwrap();

        let repo = state.repos.get(self.0.repo_id)?;
        let peer = state.peers.get(self.0.peer_id)?;

        // If two peers are on the same LAN we exchange also their local addresses. Otherwise we
        // exchange only their global addresses.
        //
        // We assume that they are on the same LAN if we are connected to both of them on at
        // least one local address. This should work in most cases except if we are connected
        // to two (or more) separate LANs. Then we would still send adresses of peers on one of
        // the LANs to peers on the other ones. Those addresses would not be useful to them but
        // apart from that it should be harmless.
        let is_global = peer.addrs.iter().all(|addr| ip::is_global(&addr.ip()));

        let addrs = repo
            .peers
            .iter()
            .filter(|peer_id| **peer_id != self.0.peer_id)
            .filter_map(|peer_id| state.peers.get(*peer_id))
            .flat_map(|peer| &peer.addrs)
            .filter(|addr| !is_global || ip::is_global(&addr.ip()))
            .copied()
            .collect();

        Some(addrs)
    }
}

impl<'a> Drop for PexCollector<'a> {
    fn drop(&mut self) {
        if let Some(repo) = self.0.state.write().unwrap().repos.get_mut(self.0.repo_id) {
            repo.peers.remove(&self.0.peer_id);
        }
    }
}

/// Receives contacts from other peers and pushes them to the peer discovery channel.
pub(crate) struct PexReceiver {
    repo_id: RepoId,
    state: Arc<RwLock<State>>,
    seen_peers: SeenPeers,
    discover_tx: mpsc::Sender<SeenPeer>,
}

impl PexReceiver {
    pub async fn handle_message(&self, payload: PexPayload) {
        if !self.is_enabled() {
            return;
        }

        for addr in payload.0 {
            if let Some(seen_peer) = self.seen_peers.insert(addr) {
                self.discover_tx.send(seen_peer).await.ok();
            }
        }
    }

    fn is_enabled(&self) -> bool {
        self.state
            .read()
            .unwrap()
            .repos
            .get(self.repo_id)
            .map(|repo| repo.enabled)
            .unwrap_or(false)
    }
}

type RepoId = usize;
type PeerId = usize;

#[derive(Default)]
struct State {
    repos: Slab<RepoState>,
    peers: Slab<PeerState>,
}

#[derive(Default)]
struct RepoState {
    // Is peer exchange enabled for this repo?
    enabled: bool,
    // Set of peers sharing this repo.
    peers: HashSet<PeerId>,
}

#[derive(Default)]
struct PeerState {
    // Set of addresses of this peer.
    addrs: HashSet<PeerAddr>,
}

#[cfg(test)]
mod tests {
    use crate::sync::DropAwaitable;

    use super::*;
    use std::{
        hash::Hash,
        net::Ipv4Addr,
        sync::atomic::{AtomicU8, Ordering},
    };

    #[tokio::test]
    async fn sanity_check() {
        let (discover_tx, _) = mpsc::channel(1);
        let discovery = PexDiscovery::new(discover_tx);

        let repo_0 = discovery.new_repository();
        let repo_1 = discovery.new_repository();

        let peer_a = discovery.new_peer();
        let addr_a = make_peer_addr();
        let close_a = DropAwaitable::new();
        peer_a.handle_connection(addr_a, close_a.subscribe());

        let peer_b = discovery.new_peer();
        let addr_b = make_peer_addr();
        let close_b = DropAwaitable::new();
        peer_b.handle_connection(addr_b, close_b.subscribe());

        let peer_c = discovery.new_peer();
        let addr_c = make_peer_addr();
        let close_c = DropAwaitable::new();
        peer_c.handle_connection(addr_c, close_c.subscribe());

        let (a0, _) = peer_a.new_link(&repo_0);
        let (b0, _) = peer_b.new_link(&repo_0);
        let (c1, _) = peer_b.new_link(&repo_1);

        let a0_collector = a0.enable().unwrap();
        let b0_collector = b0.enable().unwrap();
        let c1_collector = c1.enable().unwrap();

        assert_eq!(a0_collector.collect().unwrap(), into_set([addr_b]));
        assert_eq!(b0_collector.collect().unwrap(), into_set([addr_a]));
        assert_eq!(c1_collector.collect().unwrap(), into_set([]));
    }

    fn make_peer_addr() -> PeerAddr {
        static NEXT_OCTET: AtomicU8 = AtomicU8::new(1);
        PeerAddr::Quic(
            (
                Ipv4Addr::new(127, 0, 0, NEXT_OCTET.fetch_add(1, Ordering::Relaxed)),
                22222,
            )
                .into(),
        )
    }

    fn into_set<T: Eq + Hash, const N: usize>(items: [T; N]) -> HashSet<T> {
        items.into_iter().collect()
    }
}
