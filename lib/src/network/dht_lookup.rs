use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};

use btdht::InfoHash;
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::{
    DhtDiscovery, PeerAddr,
    dht_discovery::{DhtEvent, LookupRequest},
};

/// Stream returned from [`Network::dht_lookup`].
pub struct DhtLookup {
    request: Option<LookupRequest>,
    event_rx: UnboundedReceiverStream<DhtEvent>,
}

impl DhtLookup {
    pub(super) fn start(dht: &DhtDiscovery, info_hash: InfoHash, announce: bool) -> Self {
        let (peer_tx, peer_rx) = mpsc::unbounded_channel();
        let request = dht.start_lookup(info_hash, announce, peer_tx);

        Self {
            request: Some(request),
            event_rx: UnboundedReceiverStream::new(peer_rx),
        }
    }
}

impl Stream for DhtLookup {
    type Item = PeerAddr;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Some(event) = ready!(self.event_rx.poll_next_unpin(cx)) {
            match event {
                DhtEvent::PeerFound(peer) => {
                    if let Some(addr) = peer.addr_if_seen() {
                        return Poll::Ready(Some(*addr));
                    }
                }
                DhtEvent::RoundEnded => break,
            }
        }

        // Stop the lookup after one round.
        self.request.take();

        Poll::Ready(None)
    }
}
