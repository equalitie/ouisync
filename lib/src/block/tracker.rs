use super::BlockId;
use slab::Slab;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as BlockingMutex},
};
use tokio::sync::watch;

/// Helper for tracking required missing blocks.
#[derive(Clone)]
pub(crate) struct BlockTracker {
    shared: Arc<Shared>,
}

impl BlockTracker {
    /// Create block tracker with lazy block request mode.
    pub fn lazy() -> Self {
        Self::new(Mode::Lazy)
    }

    /// Create block tracker with greedy block request mode.
    pub fn greedy() -> Self {
        Self::new(Mode::Greedy)
    }

    fn new(mode: Mode) -> Self {
        let (notify_tx, _) = watch::channel(());

        Self {
            shared: Arc::new(Shared {
                mode,
                inner: BlockingMutex::new(Inner {
                    missing_blocks: HashMap::new(),
                    clients: Slab::new(),
                }),
                notify_tx,
            }),
        }
    }

    /// Mark the block with the given id as required.
    ///
    /// # Panics
    ///
    /// Panics if this tracker is in greedy mode.
    pub fn require(&self, block_id: BlockId) {
        assert!(
            matches!(self.shared.mode, Mode::Lazy),
            "`require` can be called only in lazy mode"
        );

        // tracing::debug!(?block_id, "require");

        let mut inner = self.shared.inner.lock().unwrap();

        let missing_block = inner
            .missing_blocks
            .entry(block_id)
            .or_insert_with(|| MissingBlock {
                offers: HashSet::new(),
                state: MissingBlockState::Required,
            });

        if missing_block.state.switch_offered_to_required() {
            self.shared.notify();
        }
    }

    /// Mark the block request as successfuly completed.
    pub fn complete(&self, block_id: &BlockId) {
        // tracing::debug!(?block_id, "complete");

        let mut inner = self.shared.inner.lock().unwrap();

        let missing_block = if let Some(missing_block) = inner.missing_blocks.remove(block_id) {
            missing_block
        } else {
            return;
        };

        for client_id in missing_block.offers {
            if let Some(block_ids) = inner.clients.get_mut(client_id) {
                block_ids.remove(block_id);
            }
        }
    }

    pub fn client(&self) -> BlockTrackerClient {
        let client_id = self
            .shared
            .inner
            .lock()
            .unwrap()
            .clients
            .insert(HashSet::new());

        let notify_rx = self.shared.notify_tx.subscribe();

        BlockTrackerClient {
            shared: self.shared.clone(),
            client_id,
            notify_rx,
        }
    }
}

pub(crate) struct BlockTrackerClient {
    shared: Arc<Shared>,
    client_id: ClientId,
    notify_rx: watch::Receiver<()>,
}

impl BlockTrackerClient {
    /// Offer to request the given block if it is, or will become, required.
    pub fn offer(&self, block_id: BlockId) {
        // tracing::debug!(?block_id, "offer");

        let mut inner = self.shared.inner.lock().unwrap();

        if !inner.clients[self.client_id].insert(block_id) {
            // Already offered
            return;
        }

        let missing_block = inner
            .missing_blocks
            .entry(block_id)
            .or_insert_with(|| MissingBlock {
                offers: HashSet::new(),
                state: MissingBlockState::Offered,
            });

        missing_block.offers.insert(self.client_id);

        match self.shared.mode {
            Mode::Greedy => {
                if missing_block.state.switch_offered_to_required() {
                    self.shared.notify()
                }
            }
            Mode::Lazy => (),
        }
    }

    /// Cancel a previously accepted request so it can be attempted by another client.
    pub fn cancel(&self, block_id: &BlockId) {
        let mut inner = self.shared.inner.lock().unwrap();

        if !inner.clients[self.client_id].remove(block_id) {
            return;
        }

        // unwrap is ok because of the invariant in `Inner`
        let missing_block = inner.missing_blocks.get_mut(block_id).unwrap();

        missing_block.offers.remove(&self.client_id);

        if missing_block
            .state
            .switch_accepted_to_required(self.client_id)
        {
            self.shared.notify();
        }
    }

    /// Returns the next required and offered block request. If there is no such request at the
    /// moment this function is called, waits until one appears.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe.
    pub async fn accept(&mut self) -> BlockId {
        loop {
            if let Some(block_id) = self.try_accept() {
                return block_id;
            }

            // unwrap is ok because the sender exists in self.shared.
            self.notify_rx.changed().await.unwrap();
        }
    }

    /// Returns the next required and offered block request or `None` if there is no such request
    /// currently.
    pub fn try_accept(&self) -> Option<BlockId> {
        let mut inner = self.shared.inner.lock().unwrap();
        let inner = &mut *inner;

        // TODO: OPTIMIZE (but profile first) this linear lookup
        for block_id in &inner.clients[self.client_id] {
            // unwrap is ok because of the invariant in `Inner`
            let missing_block = inner.missing_blocks.get_mut(block_id).unwrap();

            if missing_block
                .state
                .switch_required_to_accepted(self.client_id)
            {
                return Some(*block_id);
            }
        }

        None
    }
}

impl Drop for BlockTrackerClient {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        let block_ids = inner.clients.remove(self.client_id);
        let mut notify = false;

        for block_id in block_ids {
            // unwrap is ok because of the invariant in `Inner`
            let missing_block = inner.missing_blocks.get_mut(&block_id).unwrap();

            missing_block.offers.remove(&self.client_id);

            if missing_block
                .state
                .switch_accepted_to_required(self.client_id)
            {
                notify = true;
            }
        }

        if notify {
            self.shared.notify()
        }
    }
}

struct Shared {
    mode: Mode,
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
//     missing_blocks[block_id].offers.contains(client_id)
//
// it must hold that
//
//     clients[client_id].contains(block_id)
//
// and vice-versa.
struct Inner {
    missing_blocks: HashMap<BlockId, MissingBlock>,
    clients: Slab<HashSet<BlockId>>,
}

struct MissingBlock {
    offers: HashSet<ClientId>,
    state: MissingBlockState,
}

enum MissingBlockState {
    Offered,
    Required,
    Accepted(ClientId),
}

impl MissingBlockState {
    fn switch_offered_to_required(&mut self) -> bool {
        match self {
            Self::Offered => {
                *self = Self::Required;
                true
            }
            Self::Required | Self::Accepted(_) => false,
        }
    }

    fn switch_required_to_accepted(&mut self, acceptor_id: ClientId) -> bool {
        match self {
            Self::Required => {
                *self = Self::Accepted(acceptor_id);
                true
            }
            Self::Offered | Self::Accepted(_) => false,
        }
    }

    fn switch_accepted_to_required(&mut self, acceptor_id: ClientId) -> bool {
        match self {
            Self::Accepted(client_id) if *client_id == acceptor_id => {
                *self = Self::Required;
                true
            }
            Self::Accepted(_) | Self::Offered | Self::Required => false,
        }
    }
}

type ClientId = usize;

#[derive(Copy, Clone)]
enum Mode {
    // Blocks are downloaded only when needed.
    Lazy,
    // Blocks are downloaded as soon as we learn about them from the index.
    Greedy,
}

#[cfg(test)]
mod tests {
    use super::{
        super::{BlockData, BLOCK_SIZE},
        *,
    };
    use crate::test_utils;
    use futures_util::future;
    use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
    use std::collections::HashSet;
    use test_strategy::proptest;
    use tokio::{sync::Barrier, task};

    #[test]
    fn lazy_simple() {
        let tracker = BlockTracker::lazy();

        let client = tracker.client();

        // Initially no blocks are returned
        assert_eq!(client.try_accept(), None);

        // Required but not offered blocks are not returned
        let block0 = make_block();
        tracker.require(block0.id);
        assert_eq!(client.try_accept(), None);

        // Offered but not required blocks are not returned
        let block1 = make_block();
        client.offer(block1.id);
        assert_eq!(client.try_accept(), None);

        // Required + offered blocks are returned...
        client.offer(block0.id);
        assert_eq!(client.try_accept(), Some(block0.id));

        // ...but only once.
        assert_eq!(client.try_accept(), None);
    }

    #[test]
    fn greedy_simple() {
        let tracker = BlockTracker::greedy();

        let client = tracker.client();

        // Initially no blocks are returned
        assert_eq!(client.try_accept(), None);

        // Offered blocks are returned...
        let block = make_block();
        client.offer(block.id);
        assert_eq!(client.try_accept(), Some(block.id));

        // ...but only once.
        assert_eq!(client.try_accept(), None);
    }

    #[test]
    fn greedy_multiple_offers() {
        let num_blocks = 10;

        let tracker = BlockTracker::greedy();
        let client = tracker.client();

        let mut blocks: HashSet<BlockId> = rand::thread_rng()
            .sample_iter(Standard)
            .take(num_blocks)
            .collect();

        for block_id in &blocks {
            client.offer(*block_id);
        }

        for _ in 0..num_blocks {
            let block_id = client.try_accept().unwrap();
            assert!(blocks.remove(&block_id));
        }

        assert!(blocks.is_empty());
    }

    #[test]
    fn lazy_fallback_on_cancel_before_next() {
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        tracker.require(block.id);
        client0.offer(block.id);
        client1.offer(block.id);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn greedy_fallback_on_cancel_before_next() {
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn lazy_fallback_on_cancel_after_next() {
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        tracker.require(block.id);
        client0.offer(block.id);
        client1.offer(block.id);

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn greedy_fallback_on_cancel_after_next() {
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn lazy_fallback_on_client_drop_after_require_before_next() {
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        tracker.require(block.id);

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn lazy_fallback_on_client_drop_after_require_after_next() {
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        tracker.require(block.id);

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn lazy_fallback_on_client_drop_before_request() {
        let tracker = BlockTracker::lazy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        drop(client0);

        tracker.require(block.id);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn greedy_fallback_on_client_drop_before_next() {
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn greedy_fallback_on_client_drop_after_next() {
        let tracker = BlockTracker::greedy();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn race() {
        let num_clients = 10;

        let tracker = BlockTracker::greedy();
        let clients: Vec<_> = (0..num_clients).map(|_| tracker.client()).collect();

        let block = make_block();

        for client in &clients {
            client.offer(block.id);
        }

        // Make sure all clients stay alive until we are done so that any accepted requests are not
        // released prematurelly.
        let barrier = Arc::new(Barrier::new(clients.len()));

        // Run the clients in parallel
        let handles = clients.into_iter().map(|client| {
            task::spawn({
                let barrier = barrier.clone();
                async move {
                    let result = client.try_accept();
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

        let tracker = BlockTracker::lazy();
        let client = tracker.client();

        let block_ids: Vec<BlockId> = (&mut rng).sample_iter(Standard).take(num_blocks).collect();

        enum Op {
            Require,
            Offer,
        }

        let mut ops: Vec<_> = block_ids
            .iter()
            .map(|block_id| (Op::Require, *block_id))
            .chain(block_ids.iter().map(|block_id| (Op::Offer, *block_id)))
            .collect();
        ops.shuffle(&mut rng);

        for (op, block_id) in ops {
            match op {
                Op::Require => tracker.require(block_id),
                Op::Offer => client.offer(block_id),
            }
        }

        let mut accepted_block_ids = HashSet::with_capacity(block_ids.len());

        while let Some(block_id) = client.try_accept() {
            accepted_block_ids.insert(block_id);
        }

        assert_eq!(accepted_block_ids.len(), block_ids.len());

        for block_id in &block_ids {
            assert!(accepted_block_ids.contains(block_id));
        }
    }

    fn make_block() -> BlockData {
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        rand::thread_rng().fill(&mut content[..]);

        BlockData::from(content)
    }
}
