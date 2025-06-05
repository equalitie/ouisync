//! Peer exchange - a mechanism by which peers exchange information about other peers with each
//! other in order to discover new peers.

use super::{
    connection::ConnectionDirection,
    ip,
    message::Message,
    peer_addr::PeerAddr,
    seen_peers::{SeenPeer, SeenPeers},
    PeerSource,
};
use crate::{
    collections::HashSet,
    sync::{AwaitDrop, WatchSenderExt},
};
use rand::Rng;
use scoped_task::ScopedJoinHandle;
use serde::{Deserialize, Serialize};
use slab::Slab;
use std::{ops::Range, time::Duration};
use tokio::{
    sync::{mpsc, watch},
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
    state: watch::Sender<State>,
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
            state: watch::Sender::new(State::default()),
            seen_peers,
            discover_tx,
            _round_task: round_task,
        }
    }

    /// Sets whether sending contacts to other peers is enabled.
    pub fn set_send_enabled(&self, enabled: bool) {
        self.state.send_if_modified(|state| {
            if state.send_enabled != enabled {
                state.send_enabled = enabled;
                true
            } else {
                false
            }
        });
    }

    pub fn is_send_enabled(&self) -> bool {
        self.state.borrow().send_enabled
    }

    /// Sets whether receiving contacts from other peers is enabled.
    pub fn set_recv_enabled(&self, enabled: bool) {
        self.state.send_if_modified(|state| {
            if state.recv_enabled != enabled {
                state.recv_enabled = enabled;
                true
            } else {
                false
            }
        });
    }

    pub fn is_recv_enabled(&self) -> bool {
        self.state.borrow().recv_enabled
    }

    pub fn new_peer(&self) -> PexPeer {
        let peer_id = self
            .state
            .send_modify_return(|state| state.peers.insert(PeerState::default()));

        PexPeer {
            peer_id,
            state: self.state.clone(),
            seen_peers: self.seen_peers.clone(),
            discover_tx: self.discover_tx.clone(),
        }
    }

    pub fn new_repository(&self) -> PexRepository {
        let repo_id = self
            .state
            .send_modify_return(|state| state.repos.insert(RepoState::default()));

        PexRepository {
            repo_id,
            state: self.state.clone(),
        }
    }
}

/// Handle to manage the peer exchange for a single repository.
pub(crate) struct PexRepository {
    repo_id: RepoId,
    state: watch::Sender<State>,
}

impl PexRepository {
    pub fn is_enabled(&self) -> bool {
        self.state.borrow().repos[self.repo_id].enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.state.send_modify(|state| {
            state.repos[self.repo_id].enabled = enabled;
        });
    }
}

impl Drop for PexRepository {
    fn drop(&mut self) {
        self.state.send_modify(|state| {
            state.repos.remove(self.repo_id);
        });
    }
}

/// Handle to manage peer exchange for a single peer.
pub(crate) struct PexPeer {
    peer_id: PeerId,
    state: watch::Sender<State>,
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

        let inserted = self
            .state
            .send_if_modified(|state| state.peers[self.peer_id].addrs.insert(addr));

        if !inserted {
            // Already added
            return;
        }

        // Spawn a task that removes the address when the connection gets closed.
        task::spawn({
            let peer_id = self.peer_id;
            let state = self.state.clone();

            async move {
                closed.await;

                state.send_if_modified(|state| {
                    if let Some(peer) = state.peers.get_mut(peer_id) {
                        peer.addrs.remove(&addr)
                    } else {
                        false
                    }
                });
            }
        });
    }

    /// Creates a pair of (sender, receiver) to manage peer exchange for a single link.
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
        self.state.send_modify(|state| {
            state.peers.remove(self.peer_id);
        });
    }
}

/// Sends contacts of other peers to this peer.
pub(crate) struct PexSender {
    repo_id: RepoId,
    peer_id: PeerId,
    state: watch::Sender<State>,
}

impl PexSender {
    /// While this method is running, it periodically sends contacts of other peers that share the
    /// same repo to this peer and makes the contacts of this peer aailable to them.
    pub async fn run(&mut self, message_tx: mpsc::Sender<Message>) {
        let Some(collector) = self.enable() else {
            // Another collector for this link already exists.
            return;
        };

        loop {
            let addrs = match collector.collect() {
                Ok(addrs) => addrs,
                Err(CollectError::Disabled) => {
                    self.state
                        .subscribe()
                        .wait_for(|state| state.is_send_enabled_for(self.repo_id))
                        .await
                        .ok();
                    continue;
                }
                Err(CollectError::Closed) => break,
            };

            if !addrs.is_empty() {
                message_tx.send(Message::Pex(PexPayload(addrs))).await.ok();
            }

            let interval = rand::thread_rng().gen_range(SEND_INTERVAL_RANGE);
            time::sleep(interval).await;
        }
    }

    /// Makes the contacts of this peers available to other peers that share tha same repo as long
    /// as the returned `PexCollector` is in scope. Returns `None` if a collector for this link
    /// already exists.
    fn enable(&self) -> Option<PexCollector<'_>> {
        self.state.send_modify_return(|state| {
            if state
                .repos
                .get_mut(self.repo_id)?
                .peers
                .insert(self.peer_id)
            {
                Some(PexCollector(self))
            } else {
                None
            }
        })
    }
}

/// Collects contacts of other peers to be sent to this peer.
pub(crate) struct PexCollector<'a>(&'a PexSender);

impl PexCollector<'_> {
    fn collect(&self) -> Result<HashSet<PeerAddr>, CollectError> {
        let state = self.0.state.borrow();

        let repo = state
            .repos
            .get(self.0.repo_id)
            .ok_or(CollectError::Closed)?;
        let peer = state
            .peers
            .get(self.0.peer_id)
            .ok_or(CollectError::Closed)?;

        if !state.send_enabled || !repo.enabled {
            return Err(CollectError::Disabled);
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

impl Drop for PexCollector<'_> {
    fn drop(&mut self) {
        self.0.state.send_modify(|state| {
            if let Some(repo) = state.repos.get_mut(self.0.repo_id) {
                repo.peers.remove(&self.0.peer_id);
            }
        });
    }
}

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum CollectError {
    /// All the connections to the peer have been closed or the repository has been unlinked.
    ///
    /// Note this error should not happen in practice because in both of those cases the link should
    /// be removed which removes the `PexSender` as well and so there should be no way to call
    /// `collect` in the first place.
    Closed,
    /// Peer exchange is disabled for the repository.
    Disabled,
}

/// Receives contacts from other peers and pushes them to the peer discovery channel.
pub(crate) struct PexReceiver {
    repo_id: RepoId,
    state: watch::Sender<State>,
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
        self.state.borrow().is_recv_enabled_for(self.repo_id)
    }
}

type RepoId = usize;
type PeerId = usize;

struct State {
    repos: Slab<RepoState>,
    peers: Slab<PeerState>,
    // Whether peer contacts are sent to other peers.
    send_enabled: bool,
    // Whether peer contacts are received from peers.
    recv_enabled: bool,
}

impl State {
    fn is_send_enabled_for(&self, repo_id: RepoId) -> bool {
        self.send_enabled && self.is_repo_enabled(repo_id)
    }

    fn is_recv_enabled_for(&self, repo_id: RepoId) -> bool {
        self.recv_enabled && self.is_repo_enabled(repo_id)
    }

    fn is_repo_enabled(&self, repo_id: RepoId) -> bool {
        self.repos
            .get(repo_id)
            .map(|repo| repo.enabled)
            .unwrap_or(false)
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            repos: Slab::default(),
            peers: Slab::default(),
            send_enabled: true,
            recv_enabled: true,
        }
    }
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
    use tokio::sync::mpsc::error::TryRecvError;

    use crate::sync::DropAwaitable;

    use super::*;
    use std::{
        hash::Hash,
        iter,
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

        let (peer_a, addr_a, _close_a) = make_peer(&discovery);
        let (peer_b, addr_b, _close_b) = make_peer(&discovery);
        let (peer_c, _addr_c, _close_c) = make_peer(&discovery);

        let (a0, _) = peer_a.new_link(&repo_0);
        let (b0, _) = peer_b.new_link(&repo_0);
        let (c1, _) = peer_c.new_link(&repo_1);

        let a0_collector = a0.enable().unwrap();
        let b0_collector = b0.enable().unwrap();
        let c1_collector = c1.enable().unwrap();

        assert_eq!(a0_collector.collect().unwrap(), into_set([addr_b]));
        assert_eq!(b0_collector.collect().unwrap(), into_set([addr_a]));
        assert_eq!(c1_collector.collect().unwrap(), into_set([]));
    }

    #[tokio::test]
    async fn config() {
        let (discover_tx, mut discover_rx) = mpsc::channel(1);
        let discovery = PexDiscovery::new(discover_tx);
        let repo = discovery.new_repository();

        let (peer_a, _addr_a, _close_a) = make_peer(&discovery);
        let (peer_b, addr_b, _close_b) = make_peer(&discovery);
        let addr_c = make_peer_addr();

        let (tx_a, rx_a) = peer_a.new_link(&repo);
        let (tx_b, _rx_b) = peer_b.new_link(&repo);

        for (send_enabled, recv_enabled, repo_enabled, expected_send, expected_recv) in [
            (
                false,
                false,
                false,
                Err(CollectError::Disabled),
                Err(TryRecvError::Empty),
            ),
            (
                false,
                false,
                true,
                Err(CollectError::Disabled),
                Err(TryRecvError::Empty),
            ),
            (
                false,
                true,
                false,
                Err(CollectError::Disabled),
                Err(TryRecvError::Empty),
            ),
            (false, true, true, Err(CollectError::Disabled), Ok(addr_c)),
            (
                true,
                false,
                false,
                Err(CollectError::Disabled),
                Err(TryRecvError::Empty),
            ),
            (true, false, true, Ok(addr_b), Err(TryRecvError::Empty)),
            (
                true,
                true,
                false,
                Err(CollectError::Disabled),
                Err(TryRecvError::Empty),
            ),
            (true, true, true, Ok(addr_b), Ok(addr_c)),
        ] {
            discovery.set_send_enabled(send_enabled);
            discovery.set_recv_enabled(recv_enabled);
            repo.set_enabled(repo_enabled);

            let collector_a = tx_a.enable().unwrap();
            let _collector_b = tx_b.enable().unwrap();

            assert_eq!(
                collector_a.collect(),
                expected_send.map(iter::once).map(into_set),
                "send_enabled: {:?}, recv_enabled: {:?}, repo_enabled: {:?}",
                discovery.is_send_enabled(),
                discovery.is_recv_enabled(),
                repo.is_enabled(),
            );

            rx_a.handle_message(PexPayload(into_set([addr_c]))).await;
            assert_eq!(
                discover_rx
                    .try_recv()
                    .map(|seen_peer| *seen_peer.initial_addr()),
                expected_recv,
                "send_enabled: {:?}, recv_enabled: {:?}, repo_enabled: {:?}",
                discovery.is_send_enabled(),
                discovery.is_recv_enabled(),
                repo.is_enabled(),
            );
        }
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

    fn make_peer(discovery: &PexDiscovery) -> (PexPeer, PeerAddr, DropAwaitable) {
        let peer = discovery.new_peer();
        let addr = make_peer_addr();
        let close = DropAwaitable::new();
        peer.handle_connection(addr, PeerSource::Dht, close.subscribe());

        (peer, addr, close)
    }

    fn into_set<T>(items: T) -> HashSet<T::Item>
    where
        T: IntoIterator,
        T::Item: Eq + Hash,
    {
        items.into_iter().collect()
    }
}
