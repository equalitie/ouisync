use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};

use btdht::InfoHash;
use futures_util::{Stream, StreamExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::{DhtDiscovery, PeerAddr, dht_discovery::LookupRequest, seen_peers::SeenPeer};

/// Stream of peers discovered on the DHT
pub struct DhtLookup {
    #[expect(dead_code)]
    request: LookupRequest,
    peer_rx: UnboundedReceiverStream<SeenPeer>,
}

impl DhtLookup {
    pub(super) fn start(dht: &DhtDiscovery, info_hash: InfoHash, announce: bool) -> Self {
        let (peer_tx, peer_rx) = mpsc::unbounded_channel();
        let request = dht.start_lookup(info_hash, announce, peer_tx);

        Self {
            request,
            peer_rx: UnboundedReceiverStream::new(peer_rx),
        }
    }
}

impl Stream for DhtLookup {
    type Item = PeerAddr;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        while let Some(peer) = ready!(self.peer_rx.poll_next_unpin(cx)) {
            if let Some(addr) = peer.addr_if_seen() {
                return Poll::Ready(Some(*addr));
            }
        }

        Poll::Ready(None)
    }
}
