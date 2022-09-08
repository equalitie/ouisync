//! Peer exchange - a mechanism by which peers exchange information about other peers with each
//! other in order to discover new peers.

use super::{
    message::Content,
    message_dispatcher::PeerAddrLiveSet,
    peer_addr::PeerAddr,
    runtime_id::PublicRuntimeId,
    seen_peers::{SeenPeer, SeenPeers},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{select, sync::mpsc, time};

// TODO: add ability to enable/disable the PEX
// TODO: don't announce LAN addresses
// TODO: don't announce recently announced addresses
// TODO: announce only random subset of the addresses
// TODO: figure out when to start new round on the `SeenPeers`.
// TODO: bump the protocol version!

const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct PexPayload(HashSet<PeerAddr>);

/// Utility to retrieve contacts discovered via the peer exchange.
pub(super) struct PexDiscovery {
    rx: mpsc::Receiver<PexPayload>,
    seen_peers: SeenPeers,
}

impl PexDiscovery {
    pub fn new(rx: mpsc::Receiver<PexPayload>) -> Self {
        Self {
            rx,
            seen_peers: SeenPeers::new(),
        }
    }

    pub async fn recv(&mut self) -> Option<SeenPeer> {
        let mut addrs = Vec::new();

        loop {
            let addr = if let Some(addr) = addrs.pop() {
                addr
            } else {
                addrs = self.rx.recv().await?.0.into_iter().collect();
                continue;
            };

            if let Some(peer) = self.seen_peers.insert(addr) {
                return Some(peer);
            }
        }
    }
}

/// Group of `PexAnnouncer`s associated with a single repository. Use [`Self::bind`] to obtain an
/// individual `PexAnnouncer` for announcing only to a specific peer.
pub(super) struct PexAnnouncerGroup {
    contacts: Arc<Mutex<ContactSet>>,
}

impl PexAnnouncerGroup {
    pub fn new() -> Self {
        Self {
            contacts: Arc::new(Mutex::new(ContactSet::new())),
        }
    }

    pub fn bind(&self, peer_id: PublicRuntimeId, peer_addrs: PeerAddrLiveSet) -> PexAnnouncer {
        self.contacts.lock().unwrap().insert(peer_id, peer_addrs);

        PexAnnouncer {
            peer_id,
            contacts: self.contacts.clone(),
        }
    }
}

/// Utility to announce known contacts to a specific peer.
pub(super) struct PexAnnouncer {
    peer_id: PublicRuntimeId,
    contacts: Arc<Mutex<ContactSet>>,
}

impl PexAnnouncer {
    /// Periodically announces known peer contacts to the bound peer. Runs until the `content_tx`
    /// channel gets closed.
    pub async fn run(&self, content_tx: mpsc::Sender<Content>) {
        loop {
            select! {
                _ = time::sleep(ANNOUNCE_INTERVAL) => (),
                _ = content_tx.closed() => break,
            }

            let contacts = self.contacts.lock().unwrap().collect_for(&self.peer_id);

            tracing::trace!(?contacts, "announce");

            let content = Content::Pex(PexPayload(contacts));
            content_tx.send(content).await.ok();
        }
    }
}

impl Drop for PexAnnouncer {
    fn drop(&mut self) {
        self.contacts.lock().unwrap().remove(&self.peer_id);
    }
}

#[derive(Default)]
struct ContactSet(HashMap<PublicRuntimeId, PeerAddrLiveSet>);

impl ContactSet {
    fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, peer_id: PublicRuntimeId, peer_addrs: PeerAddrLiveSet) {
        self.0.insert(peer_id, peer_addrs);
    }

    fn remove(&mut self, peer_id: &PublicRuntimeId) {
        self.0.remove(peer_id);
    }

    fn collect_for(&self, recipient_id: &PublicRuntimeId) -> HashSet<PeerAddr> {
        self.0
            .iter()
            .filter(|(peer_id, _)| *peer_id != recipient_id)
            .flat_map(|(_, peer_addrs)| peer_addrs.collect())
            .collect()
    }
}
