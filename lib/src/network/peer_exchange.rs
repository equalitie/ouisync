//! Peer exchange - a mechanism by which peers exchange information about other peers with each
//! other in order to discover new peers.

use crate::sync::uninitialized_watch;

use super::{
    connection::ConnectionDirection,
    ip,
    message::Content,
    message_dispatcher::LiveConnectionInfoSet,
    peer_addr::PeerAddr,
    runtime_id::PublicRuntimeId,
    seen_peers::{SeenPeer, SeenPeers},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::{select, sync::mpsc};

// TODO: add ability to enable/disable the PEX
// TODO: don't announce recently announced addresses
// TODO: announce only random subset of the addresses
// TODO: figure out when to start new round on the `SeenPeers`.
// TODO: bump the protocol version!
// TODO: don't use ANNOUNCE_INTERVAL, announce immediately when a new connection is established/lost

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
    // Notified when the global peer set changes.
    peer_rx: uninitialized_watch::Receiver<()>,
    // Notified when a new link is created in this group.
    link_tx: uninitialized_watch::Sender<()>,
}

impl PexAnnouncerGroup {
    pub fn new(peer_rx: uninitialized_watch::Receiver<()>) -> Self {
        let (link_tx, _) = uninitialized_watch::channel();

        Self {
            contacts: Arc::new(Mutex::new(ContactSet::new())),
            peer_rx,
            link_tx,
        }
    }

    pub fn bind(
        &self,
        peer_id: PublicRuntimeId,
        connections: LiveConnectionInfoSet,
    ) -> PexAnnouncer {
        self.contacts.lock().unwrap().insert(peer_id, connections);
        self.link_tx.send(()).ok();

        PexAnnouncer {
            peer_id,
            contacts: self.contacts.clone(),
            peer_rx: self.peer_rx.clone(),
            link_rx: self.link_tx.subscribe(),
        }
    }
}

/// Utility to announce known contacts to a specific peer.
pub(super) struct PexAnnouncer {
    peer_id: PublicRuntimeId,
    contacts: Arc<Mutex<ContactSet>>,
    peer_rx: uninitialized_watch::Receiver<()>,
    link_rx: uninitialized_watch::Receiver<()>,
}

impl PexAnnouncer {
    /// Periodically announces known peer contacts to the bound peer. Runs until the `content_tx`
    /// channel gets closed.
    pub async fn run(&mut self, content_tx: mpsc::Sender<Content>) {
        loop {
            select! {
                result = self.peer_rx.changed() => {
                    if result.is_err() {
                        // The `ConnectionDeduplicator` has been destroyed which means everything is
                        // shutting down.
                        break;
                    }
                }
                result = self.link_rx.changed() => {
                    if result.is_err() {
                        // The repository has been unregistered.
                        break;
                    }
                }
                _ = content_tx.closed() => {
                    // The connection to the current peer has been terminated.
                    break;
                }
            }

            let contacts: HashSet<_> = self
                .contacts
                .lock()
                .unwrap()
                .iter_for(&self.peer_id)
                .collect();
            if contacts.is_empty() {
                continue;
            }

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
struct ContactSet(HashMap<PublicRuntimeId, LiveConnectionInfoSet>);

impl ContactSet {
    fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, peer_id: PublicRuntimeId, connections: LiveConnectionInfoSet) {
        self.0.insert(peer_id, connections);
    }

    fn remove(&mut self, peer_id: &PublicRuntimeId) {
        self.0.remove(peer_id);
    }

    fn iter_for<'a>(
        &'a self,
        recipient_id: &'a PublicRuntimeId,
    ) -> impl Iterator<Item = PeerAddr> + 'a {
        // If the recipient is local, we send them all known contacts - global and local. If they
        // are global, we send them only global contacts. A peer is considered local for this
        // purpose if at least one of their addresses is local.
        let is_local = if let Some(connections) = self.0.get(recipient_id) {
            connections
                .iter()
                .any(|info| !ip::is_global(&info.addr.ip()))
        } else {
            false
        };

        self.0
            .iter()
            .filter(move |(peer_id, _)| *peer_id != recipient_id)
            .flat_map(move |(_, connections)| {
                connections
                    .iter()
                    .filter(move |info| is_local || ip::is_global(&info.addr.ip()))
                    // Filter out incoming TCP contacts because they can't be used to establish
                    // outgoing connection.
                    .filter(|info| !info.addr.is_tcp() || info.dir == ConnectionDirection::Incoming)
            })
            .map(|info| info.addr)
    }
}
