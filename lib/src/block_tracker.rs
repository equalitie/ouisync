use crate::{
    collections::{HashMap, HashSet},
    protocol::BlockId,
};
use deadlock::BlockingMutex;
use std::{collections::hash_map::Entry, sync::Arc};
use tokio::sync::watch;

/// Helper for tracking required missing blocks.
#[derive(Clone)]
pub(crate) struct BlockTracker {
    shared: Arc<Shared>,
}

impl BlockTracker {
    pub fn new() -> Self {
        let (notify_tx, _) = watch::channel(());

        Self {
            shared: Arc::new(Shared {
                inner: BlockingMutex::new(Inner {
                    missing_blocks: HashMap::default(),
                    clients: HashMap::default(),
                    next_client_id: 0,
                }),
                notify_tx,
            }),
        }
    }

    /// Mark the block with the given id as required.
    pub fn require(&self, block_id: BlockId) {
        if self.shared.inner.lock().unwrap().require(block_id) {
            self.shared.notify()
        }
    }

    pub fn require_batch(&self) -> RequireBatch<'_> {
        RequireBatch {
            shared: &self.shared,
            notify: false,
        }
    }

    /// Approve the block request if offered. This is called when `quota` is not `None`, otherwise
    /// blocks are pre-approved from `TrackerClient::register(block_id, OfferState::Approved)`.
    pub fn approve(&self, block_id: BlockId) {
        let mut inner = self.shared.inner.lock().unwrap();

        let Some(missing_block) = inner.missing_blocks.get_mut(&block_id) else {
            return;
        };

        let required = match &mut missing_block.state {
            State::Idle { approved: true, .. } | State::Accepted(_) => return,
            State::Idle { approved, required } => {
                *approved = true;
                *required
            }
        };

        // If required and offered, notify the waiting acceptors.
        if required && !missing_block.offers.is_empty() {
            self.shared.notify();
        }
    }

    pub fn client(&self) -> TrackerClient {
        let client_id = self.shared.inner.lock().unwrap().insert_client();
        let notify_rx = self.shared.notify_tx.subscribe();

        TrackerClient {
            shared: self.shared.clone(),
            client_id,
            notify_rx,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum OfferState {
    Pending,
    Approved,
}

pub(crate) struct RequireBatch<'a> {
    shared: &'a Shared,
    notify: bool,
}

impl RequireBatch<'_> {
    pub fn add(&mut self, block_id: BlockId) {
        if self.shared.inner.lock().unwrap().require(block_id) {
            self.notify = true;
        }
    }
}

impl Drop for RequireBatch<'_> {
    fn drop(&mut self) {
        if self.notify {
            self.shared.notify();
        }
    }
}

pub(crate) struct TrackerClient {
    shared: Arc<Shared>,
    client_id: ClientId,
    notify_rx: watch::Receiver<()>,
}

impl TrackerClient {
    /// Returns a stream of offers for required blocks.
    pub fn offers(&self) -> BlockOffers {
        BlockOffers {
            shared: self.shared.clone(),
            client_id: self.client_id,
            notify_rx: self.notify_rx.clone(),
        }
    }

    /// Registers an offer for a block with the given id.
    /// Returns `true` if this block was offered for the first time (by any client) or `false` if
    /// it's already been offered but not yet accepted or cancelled.
    pub fn register(&self, block_id: BlockId, state: OfferState) -> bool {
        let mut inner = self.shared.inner.lock().unwrap();

        // unwrap is OK because if `self` exists the `inner.clients` entry must exists as well.
        if !inner
            .clients
            .get_mut(&self.client_id)
            .unwrap()
            .insert(block_id)
        {
            // Already offered
            return false;
        }

        let missing_block = inner
            .missing_blocks
            .entry(block_id)
            .or_insert_with(|| MissingBlock {
                offers: HashMap::default(),
                state: State::Idle {
                    required: false,
                    approved: false,
                },
            });

        missing_block
            .offers
            .insert(self.client_id, Offer::Available);

        match &mut missing_block.state {
            State::Idle { approved, .. } => {
                match state {
                    OfferState::Approved => {
                        *approved = true;
                    }
                    OfferState::Pending => (),
                }

                if *approved {
                    self.shared.notify();
                }
            }
            State::Accepted(_) => (),
        }

        true
    }
}

impl Drop for TrackerClient {
    fn drop(&mut self) {
        if self
            .shared
            .inner
            .lock()
            .unwrap()
            .remove_client(self.client_id)
        {
            self.shared.notify();
        }
    }
}

/// Stream of offers for required blocks.
pub(crate) struct BlockOffers {
    shared: Arc<Shared>,
    client_id: ClientId,
    notify_rx: watch::Receiver<()>,
}

impl BlockOffers {
    /// Returns the next offer, waiting for one to appear if necessary.
    pub async fn next(&mut self) -> BlockOffer {
        loop {
            if let Some(offer) = self.try_next() {
                return offer;
            }

            // unwrap is ok because the sender exists in self.shared.
            self.notify_rx.changed().await.unwrap();
        }
    }

    /// Returns the next offer or `None` if none exists currently.
    pub fn try_next(&self) -> Option<BlockOffer> {
        let block_id = self
            .shared
            .inner
            .lock()
            .unwrap()
            .propose_offer(self.client_id)?;

        Some(BlockOffer {
            shared: self.shared.clone(),
            client_id: self.client_id,
            block_id,
            complete: false,
        })
    }
}

/// Offer for a required block.
pub(crate) struct BlockOffer {
    shared: Arc<Shared>,
    client_id: ClientId,
    block_id: BlockId,
    complete: bool,
}

impl BlockOffer {
    #[cfg(test)]
    pub fn block_id(&self) -> &BlockId {
        &self.block_id
    }

    /// Accepts the offer. There can be multiple offers for the same block (each from a different
    /// peer) but only one returns `Some` here. The returned `BlockPromise` is a commitment to send
    /// the block request through this client.
    pub fn accept(self) -> Option<BlockPromise> {
        if self
            .shared
            .inner
            .lock()
            .unwrap()
            .accept_offer(&self.block_id, self.client_id)
        {
            Some(BlockPromise(self))
        } else {
            None
        }
    }
}

impl Drop for BlockOffer {
    fn drop(&mut self) {
        if self.complete {
            return;
        }

        if self
            .shared
            .inner
            .lock()
            .unwrap()
            .cancel_offer(&self.block_id, self.client_id)
        {
            self.shared.notify();
        }
    }
}

/// Accepted block offer.
pub(crate) struct BlockPromise(BlockOffer);

impl BlockPromise {
    pub(crate) fn block_id(&self) -> &BlockId {
        &self.0.block_id
    }

    /// Mark the block request as successfully completed.
    pub fn complete(mut self) {
        self.0.complete = true;
        self.0
            .shared
            .inner
            .lock()
            .unwrap()
            .complete(&self.0.block_id);
    }
}

struct Shared {
    inner: BlockingMutex<Inner>,
    notify_tx: watch::Sender<()>,
}

impl Shared {
    fn notify(&self) {
        self.notify_tx.send(()).unwrap_or(())
    }
}

// Invariant: for all `block_id` and `client_id` such that
//
//     missing_blocks[block_id].offers.contains_key(client_id)
//
// it must hold that
//
//     clients[client_id].contains(block_id)
//
// and vice-versa.
struct Inner {
    missing_blocks: HashMap<BlockId, MissingBlock>,
    clients: HashMap<ClientId, HashSet<BlockId>>,
    next_client_id: ClientId,
}

impl Inner {
    fn insert_client(&mut self) -> ClientId {
        let client_id = self.next_client_id;
        self.next_client_id = self
            .next_client_id
            .checked_add(1)
            .expect("too many clients");
        self.clients.insert(client_id, HashSet::new());
        client_id
    }

    fn remove_client(&mut self, client_id: ClientId) -> bool {
        // unwrap is ok because if `self` exists the `clients` entry must exists as well.
        let block_ids = self.clients.remove(&client_id).unwrap();
        let mut notify = false;

        for block_id in block_ids {
            // unwrap is ok because of the invariant in `Inner`
            let missing_block = self.missing_blocks.get_mut(&block_id).unwrap();

            missing_block.offers.remove(&client_id);

            if missing_block.unaccept_by(client_id) {
                notify = true;
            }

            // TODO: if the block hasn't other offers and isn't required, remove it
        }

        notify
    }

    /// Mark the block with the given id as required. Returns true if the block wasn't already
    /// required and if it has at least one offer. Otherwise returns false.
    fn require(&mut self, block_id: BlockId) -> bool {
        let missing_block = self
            .missing_blocks
            .entry(block_id)
            .or_insert_with(|| MissingBlock {
                offers: HashMap::default(),
                state: State::Idle {
                    required: false,
                    approved: false,
                },
            });

        match &mut missing_block.state {
            State::Idle { required: true, .. } | State::Accepted(_) => false,
            State::Idle { required, .. } => {
                *required = true;
                !missing_block.offers.is_empty()
            }
        }
    }

    fn complete(&mut self, block_id: &BlockId) {
        let Some(missing_block) = self.missing_blocks.remove(block_id) else {
            return;
        };

        for (client_id, _) in missing_block.offers {
            if let Some(block_ids) = self.clients.get_mut(&client_id) {
                block_ids.remove(block_id);
            }
        }
    }

    fn propose_offer(&mut self, client_id: ClientId) -> Option<BlockId> {
        // TODO: OPTIMIZE (but profile first) this linear lookup
        for block_id in self.clients.get(&client_id).into_iter().flatten() {
            // unwrap is ok because of the invariant in `Inner`
            let missing_block = self.missing_blocks.get_mut(block_id).unwrap();

            match missing_block.state {
                State::Idle {
                    required: true,
                    approved: true,
                } => (),
                State::Idle { .. } | State::Accepted(_) => continue,
            }

            // unwrap is ok because of the invariant.
            let offer = missing_block.offers.get_mut(&client_id).unwrap();
            match offer {
                Offer::Available => {
                    *offer = Offer::Proposed;
                }
                Offer::Proposed | Offer::Accepted => continue,
            }

            return Some(*block_id);
        }

        None
    }

    fn accept_offer(&mut self, block_id: &BlockId, client_id: ClientId) -> bool {
        let Some(missing_block) = self.missing_blocks.get_mut(block_id) else {
            return false;
        };

        match missing_block.state {
            State::Idle {
                required: true,
                approved: true,
            } => (),
            State::Idle { .. } | State::Accepted(_) => return false,
        }

        missing_block.state = State::Accepted(client_id);
        missing_block.offers.insert(client_id, Offer::Accepted);

        true
    }

    fn cancel_offer(&mut self, block_id: &BlockId, client_id: ClientId) -> bool {
        let Some(missing_block) = self.missing_blocks.get_mut(block_id) else {
            return false;
        };

        let Entry::Occupied(mut entry) = missing_block.offers.entry(client_id) else {
            return false;
        };

        match entry.get() {
            Offer::Proposed => {
                entry.insert(Offer::Available);
            }
            Offer::Accepted => {
                // Cancelling an accepted offer means the request either failed or timeouted so it's
                // safe to remove it. If the peer sends us another leaf node response with the same
                // block id, we register the offer again.
                entry.remove();
                // unwrap is ok because if the client has been already destroyed then
                // `missing_block.offers[&self.client_id]` would not exists and this function would
                // have exited earlier.
                self.clients.get_mut(&client_id).unwrap().remove(block_id);
            }
            Offer::Available => unreachable!(),
        }

        missing_block.unaccept_by(client_id)
    }
}

#[derive(Debug)]
struct MissingBlock {
    // Clients that offered this block.
    offers: HashMap<ClientId, Offer>,
    state: State,
}

impl MissingBlock {
    fn unaccept_by(&mut self, client_id: ClientId) -> bool {
        match self.state {
            State::Accepted(other_client_id) if other_client_id == client_id => {
                self.state = State::Idle {
                    required: true,
                    approved: true,
                };
                true
            }
            State::Accepted(_) | State::Idle { .. } => false,
        }
    }
}

#[derive(Debug)]
enum State {
    Idle { required: bool, approved: bool },
    Accepted(ClientId),
}

#[derive(Debug)]
enum Offer {
    Available,
    Proposed,
    Accepted,
}

type ClientId = usize;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{collections::HashSet, protocol::Block, test_utils};
    use futures_util::future;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use std::{pin::pin, time::Duration};
    use test_strategy::proptest;
    use tokio::{select, sync::mpsc, sync::Barrier, task, time};

    #[test]
    fn simple() {
        let tracker = BlockTracker::new();

        let client = tracker.client();

        // Initially no blocks are returned
        assert!(client.offers().try_next().is_none());

        // Offered but not required blocks are not returned
        let block0: Block = rand::random();
        client.register(block0.id, OfferState::Approved);
        assert!(client.offers().try_next().is_none());

        // Required but not offered blocks are not returned
        let block1: Block = rand::random();
        tracker.require(block1.id);
        assert!(client.offers().try_next().is_none());

        // Required + offered blocks are returned...
        tracker.require(block0.id);
        assert_eq!(
            client
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block0.id)
        );

        // ...but only once.
        assert!(client.offers().try_next().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_async() {
        let tracker = BlockTracker::new();

        let block: Block = rand::random();
        let client = tracker.client();
        let mut offers = client.offers();

        tracker.require(block.id);

        let (tx, mut rx) = mpsc::channel(1);

        let handle = tokio::task::spawn(async move {
            let mut next = pin!(offers.next());

            loop {
                select! {
                    block_offer = &mut next => {
                        return *block_offer.block_id();
                    },
                    _ = tx.send(()) => {}
                }
            }
        });

        // Make sure the task started.
        rx.recv().await.unwrap();

        client.register(block.id, OfferState::Approved);

        let offered_block_id = time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("timeout")
            .unwrap();

        assert_eq!(block.id, offered_block_id);
    }

    #[test]
    fn fallback_on_cancel_after_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block: Block = rand::random();

        tracker.require(block.id);
        client0.register(block.id, OfferState::Approved);
        client1.register(block.id, OfferState::Approved);

        let block_promise = client0.offers().try_next().and_then(BlockOffer::accept);
        assert_eq!(
            block_promise.as_ref().map(BlockPromise::block_id),
            Some(&block.id)
        );
        assert!(client1.offers().try_next().is_none());

        drop(block_promise);

        assert!(client0.offers().try_next().is_none());
        assert_eq!(
            client1
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block.id)
        );
    }

    #[test]
    fn fallback_on_client_drop_after_require_before_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block: Block = rand::random();

        client0.register(block.id, OfferState::Approved);
        client1.register(block.id, OfferState::Approved);

        tracker.require(block.id);

        drop(client0);

        assert_eq!(
            client1
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block.id)
        );
    }

    #[test]
    fn fallback_on_client_drop_after_require_after_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block: Block = rand::random();

        client0.register(block.id, OfferState::Approved);
        client1.register(block.id, OfferState::Approved);

        tracker.require(block.id);

        let block_promise = client0.offers().try_next().and_then(BlockOffer::accept);

        assert_eq!(
            block_promise.as_ref().map(BlockPromise::block_id),
            Some(&block.id)
        );
        assert!(client1.offers().try_next().is_none());

        drop(client0);

        assert_eq!(
            client1
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block.id)
        );
    }

    #[test]
    fn fallback_on_client_drop_before_require() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block: Block = rand::random();

        client0.register(block.id, OfferState::Approved);
        client1.register(block.id, OfferState::Approved);

        drop(client0);

        tracker.require(block.id);

        assert_eq!(
            client1
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block.id)
        );
    }

    #[test]
    fn approve() {
        let tracker = BlockTracker::new();
        let client = tracker.client();

        let block: Block = rand::random();
        tracker.require(block.id);

        client.register(block.id, OfferState::Pending);
        assert!(client.offers().try_next().is_none());

        tracker.approve(block.id);
        assert_eq!(
            client
                .offers()
                .try_next()
                .and_then(BlockOffer::accept)
                .as_ref()
                .map(BlockPromise::block_id),
            Some(&block.id)
        );
    }

    #[test]
    fn multiple_offers_from_different_clients() {
        let tracker = BlockTracker::new();
        let client0 = tracker.client();
        let client1 = tracker.client();

        let block: Block = rand::random();
        tracker.require(block.id);

        client0.register(block.id, OfferState::Approved);
        client1.register(block.id, OfferState::Approved);

        let offer0 = client0.offers().try_next().unwrap();
        let offer1 = client1.offers().try_next().unwrap();

        assert_eq!(offer0.block_id(), offer1.block_id());

        let promise0 = offer0.accept();
        let promise1 = offer1.accept();

        assert!(promise0.is_some());
        assert!(promise1.is_none());
    }

    #[test]
    fn multiple_offers_from_same_client() {
        let tracker = BlockTracker::new();
        let client = tracker.client();

        let block0: Block = rand::random();
        tracker.require(block0.id);
        client.register(block0.id, OfferState::Approved);

        let block1: Block = rand::random();
        tracker.require(block1.id);
        client.register(block1.id, OfferState::Approved);

        let offer0 = client.offers().try_next().unwrap();
        let offer1 = client.offers().try_next().unwrap();
        let offer2 = client.offers().try_next();

        assert_ne!(offer0.block_id(), offer1.block_id());
        assert!(offer2.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn race() {
        let num_clients = 10;

        let tracker = BlockTracker::new();
        let clients: Vec<_> = (0..num_clients).map(|_| tracker.client()).collect();

        let block: Block = rand::random();

        tracker.require(block.id);

        for client in &clients {
            client.register(block.id, OfferState::Approved);
        }

        // Make sure all clients stay alive until we are done so that any accepted requests are not
        // released prematurely.
        let barrier = Arc::new(Barrier::new(clients.len()));

        // Run the clients in parallel
        let handles = clients.into_iter().map(|client| {
            task::spawn({
                let barrier = barrier.clone();
                async move {
                    let block_promise = client.offers().try_next().and_then(BlockOffer::accept);
                    let result = block_promise.as_ref().map(BlockPromise::block_id).cloned();
                    barrier.wait().await;
                    result
                }
            })
        });

        let block_ids = future::try_join_all(handles).await.unwrap();

        // Exactly one client gets the block id
        let mut block_ids = block_ids.into_iter().flatten();
        assert_eq!(block_ids.next(), Some(block.id));
        assert_eq!(block_ids.next(), None);
    }

    #[proptest]
    fn stress(
        #[strategy(1usize..100)] num_blocks: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        stress_case(num_blocks, rng_seed)
    }

    fn stress_case(num_blocks: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let tracker = BlockTracker::new();
        let client = tracker.client();

        let block_ids: Vec<BlockId> = (&mut rng).sample_iter(Standard).take(num_blocks).collect();

        enum Op {
            Require,
            Register,
        }

        let mut ops: Vec<_> = block_ids
            .iter()
            .map(|block_id| (Op::Require, *block_id))
            .chain(block_ids.iter().map(|block_id| (Op::Register, *block_id)))
            .collect();
        ops.shuffle(&mut rng);

        for (op, block_id) in ops {
            match op {
                Op::Require => {
                    tracker.require(block_id);
                }
                Op::Register => {
                    client.register(block_id, OfferState::Approved);
                }
            }
        }

        let mut block_promise = HashSet::with_capacity(block_ids.len());

        while let Some(block_id) = client
            .offers()
            .try_next()
            .and_then(BlockOffer::accept)
            .as_ref()
            .map(BlockPromise::block_id)
        {
            block_promise.insert(*block_id);
        }

        assert_eq!(block_promise.len(), block_ids.len());

        for block_id in &block_ids {
            assert!(block_promise.contains(block_id));
        }
    }
}
