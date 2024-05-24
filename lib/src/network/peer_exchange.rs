//! Peer exchange - a mechanism by which peers exchange information about other peers with each
//! other in order to discover new peers.

use super::{
    connection::ConnectionDirection,
    ip,
    message::Content,
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
    PeerSource,
};
use crate::{collections::HashSet, sync::AwaitDrop};
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{
    fmt,
    ops::Range,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    sync::{mpsc, Notify},
    task, time,
};

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
    state_notify: Arc<Notify>,
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
            state_notify: Arc::new(Notify::new()),
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
            state_notify: self.state_notify.clone(),
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
            state_notify: self.state_notify.clone(),
        }
    }
}

/// Handle to manage the peer exchange for a single repository.
pub(crate) struct PexRepository {
    repo_id: RepoId,
    state: Arc<RwLock<State>>,
    state_notify: Arc<Notify>,
}

impl PexRepository {
    pub fn is_enabled(&self) -> bool {
        self.state.read().unwrap().repos[self.repo_id].enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.state.write().unwrap().repos[self.repo_id].enabled = enabled;
        self.state_notify.notify_waiters();
    }
}

impl Drop for PexRepository {
    fn drop(&mut self) {
        self.state.write().unwrap().repos.remove(self.repo_id);
        self.state_notify.notify_waiters();
    }
}

/// Handle to manage peer exchange for a single peer.
pub(crate) struct PexPeer {
    peer_id: PeerId,
    state: Arc<RwLock<State>>,
    state_notify: Arc<Notify>,
    seen_peers: SeenPeers,
    discover_tx: mpsc::Sender<SeenPeer>,
}

impl PexPeer {
    /// Call this whenever new connection to this peer has been established. The `closed` should
    /// trigger when the connection gets closed.
    pub fn handle_connection(&self, addr: PeerAddr, source: PeerSource, closed: AwaitDrop) {
        if addr.is_tcp()
            && ConnectionDirection::from_source(source) == ConnectionDirection::Incoming
        {
            // Incomming TCP address can't be connected to (it has different port than the listener)
            // so there is no point exchanging it.
            return;
        }

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

    /// Creates a pair of (sender, receiver) to manage peer exchange for a single link.
    pub fn new_link(&self, repo: &PexRepository) -> (PexSender, PexReceiver) {
        let tx = PexSender {
            repo_id: repo.repo_id,
            peer_id: self.peer_id,
            state: self.state.clone(),
            state_notify: self.state_notify.clone(),
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

/// Sends contacts of other peers to this peer.
pub(crate) struct PexSender {
    repo_id: RepoId,
    peer_id: PeerId,
    state: Arc<RwLock<State>>,
    state_notify: Arc<Notify>,
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
            let addrs = match collector.collect() {
                Ok(addrs) => addrs,
                Err(CollectError::Disabled(notify)) => {
                    notify.notified().await;
                    continue;
                }
                Err(CollectError::Closed) => break,
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

/// Collects contacts of other peers to be sent to this peer.
pub(crate) struct PexCollector<'a>(&'a PexSender);

impl<'a> PexCollector<'a> {
    fn collect(&self) -> Result<HashSet<PeerAddr>, CollectError> {
        let state = self.0.state.read().unwrap();

        let repo = state
            .repos
            .get(self.0.repo_id)
            .ok_or(CollectError::Closed)?;
        let peer = state
            .peers
            .get(self.0.peer_id)
            .ok_or(CollectError::Closed)?;

        if !repo.enabled {
            return Err(CollectError::Disabled(self.0.state_notify.clone()));
        }

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

        Ok(addrs)
    }
}

impl<'a> Drop for PexCollector<'a> {
    fn drop(&mut self) {
        if let Some(repo) = self.0.state.write().unwrap().repos.get_mut(self.0.repo_id) {
            repo.peers.remove(&self.0.peer_id);
        }
    }
}

pub(crate) enum CollectError {
    /// All the connections to the peer have been closed or the repository has been unlinked.
    ///
    /// Note this error should not happen in practice because in both of those cases the link should
    /// be removed which removes the `PexSender` as well and so there should be no way to call
    /// `collect` in the first place.
    Closed,
    /// Peer exchange is disabled for the repository. The `Notify` can be used to wait until it gets
    /// enabled.
    Disabled(Arc<Notify>),
}

impl fmt::Debug for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "Closed"),
            Self::Disabled(_) => write!(f, "Disabled(_)"),
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
        repo_0.set_enabled(true);

        let repo_1 = discovery.new_repository();
        repo_1.set_enabled(true);

        let peer_a = discovery.new_peer();
        let addr_a = make_peer_addr();
        let close_a = DropAwaitable::new();
        peer_a.handle_connection(addr_a, PeerSource::Dht, close_a.subscribe());

        let peer_b = discovery.new_peer();
        let addr_b = make_peer_addr();
        let close_b = DropAwaitable::new();
        peer_b.handle_connection(addr_b, PeerSource::Dht, close_b.subscribe());

        let peer_c = discovery.new_peer();
        let addr_c = make_peer_addr();
        let close_c = DropAwaitable::new();
        peer_c.handle_connection(addr_c, PeerSource::Dht, close_c.subscribe());

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
