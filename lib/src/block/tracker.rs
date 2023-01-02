use super::BlockId;
use slab::Slab;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::{Arc, Mutex as BlockingMutex},
};
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
                    missing_blocks: HashMap::new(),
                    clients: Slab::new(),
                    complete_counter: 0,
                }),
                notify_tx,
            }),
        }
    }

    /// Begin marking the block with the given id as required. See also [`Require::commit`].
    pub fn begin_require(&self, block_id: BlockId) -> Require {
        let inner = self.shared.inner.lock().unwrap();

        Require {
            shared: self.shared.clone(),
            complete_counter: inner.complete_counter,
            block_id,
        }
    }

    /// Mark the block request as successfully completed.
    pub fn complete(&self, block_id: &BlockId) {
        tracing::trace!(?block_id, "complete");

        let mut inner = self.shared.inner.lock().unwrap();

        let missing_block = if let Some(missing_block) = inner.missing_blocks.remove(block_id) {
            missing_block
        } else {
            return;
        };

        for (client_id, _) in missing_block.clients {
            if let Some(block_ids) = inner.clients.get_mut(client_id) {
                block_ids.remove(block_id);
            }
        }

        inner.complete_counter = inner.complete_counter.wrapping_add(1);
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
    /// Offer to request the given block if it is, or will become, required. Returns `true` if this
    /// block was offered for the first time, `false` if it was already offered before but not yet
    /// accepted or cancelled.
    pub fn offer(&self, block_id: BlockId) -> bool {
        let mut inner = self.shared.inner.lock().unwrap();

        if !inner.clients[self.client_id].insert(block_id) {
            // Already offered
            return false;
        }

        tracing::trace!(?block_id, "offer");

        let missing_block = inner
            .missing_blocks
            .entry(block_id)
            .or_insert_with(|| MissingBlock {
                clients: HashMap::new(),
                state: MissingBlockState::Offered,
            });

        missing_block
            .clients
            .insert(self.client_id, ClientState::Offered);

        true
    }

    /// Cancel a previously accepted request so it can be attempted by another client.
    pub fn cancel(&self, block_id: &BlockId) {
        let mut inner = self.shared.inner.lock().unwrap();

        inner.clients[self.client_id].remove(block_id);

        let Some(missing_block) = inner.missing_blocks.get_mut(block_id) else { return };

        tracing::trace!(?block_id, "cancel");

        missing_block
            .clients
            .insert(self.client_id, ClientState::Cancelled);

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

        // First try to accept a required block offered by this client.
        // TODO: OPTIMIZE (but profile first) this linear lookup
        for block_id in &inner.clients[self.client_id] {
            // unwrap is ok because of the invariant in `Inner`
            let missing_block = inner.missing_blocks.get_mut(block_id).unwrap();

            if missing_block
                .state
                .switch_required_to_accepted(self.client_id)
            {
                tracing::trace!(?block_id, "accept offered");
                return Some(*block_id);
            }
        }

        // If none, try to accept a required block with no offers. This is because there are some
        // edge cases where a block might be required even before any replica offered it.
        //
        // Example of such edge case:
        //
        // A block is initially present, but is part of a an outdated file/directory. A new snapshot
        // is in the process of being downloaded from a remote replica. During this download, the
        // block is still present and so is not marked as offered (because at least one of its
        // local ancestor nodes is still seen as up-to-date). Then before the download completes,
        // the worker garbage-collects the block. Then the download completes and triggers another
        // worker run. During this run the block might be marked as required again (because e.g.
        // the file was modified by the remote replica). But the block hasn't been marked as
        // offered (because it was still present during the last snapshot download) and so is not
        // requested. We now have to wait for the next snapshot update from the remote replica
        // before the block is marked as offered and only then we proceed with requesting it. This
        // can take arbitrarily long (even indefinitely).
        //
        // By accepting also non-offered blocks, we ensure that the missing block is requested as
        // soon as possible.
        //
        // One downside of this is we might send block requests to replicas that don't have then,
        // wasting some traffic. This situation should not happen too often so this is considered
        // acceptable.
        'outer: for (block_id, missing_block) in &mut inner.missing_blocks {
            for (client_id, state) in &missing_block.clients {
                match state {
                    ClientState::Offered => {
                        // This block has offers
                        continue 'outer;
                    }
                    ClientState::Cancelled if *client_id == self.client_id => {
                        // This block was already requested by this client and cancelled. No point
                        // trying it again.
                        continue 'outer;
                    }
                    ClientState::Cancelled => (),
                }
            }

            if missing_block
                .state
                .switch_required_to_accepted(self.client_id)
            {
                tracing::trace!(?block_id, "accept unoffered");
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

            missing_block.clients.remove(&self.client_id);

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

pub(crate) struct Require {
    shared: Arc<Shared>,
    block_id: BlockId,
    complete_counter: u64,
}

impl Require {
    /// Commits marking the block as required. When this returns `true`, the block was successfully
    /// marked as required. Otherwise the block might have been completed after `begin_require` was
    /// called (by some other task) and the marking needs to be restarted.
    pub fn commit(self) -> bool {
        let mut inner = self.shared.inner.lock().unwrap();

        if inner.complete_counter != self.complete_counter {
            return false;
        }

        tracing::trace!(block_id = ?self.block_id, "require");

        match inner.missing_blocks.entry(self.block_id) {
            Entry::Occupied(mut entry) => {
                if entry.get_mut().state.switch_offered_to_required() {
                    self.shared.notify();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(MissingBlock {
                    clients: HashMap::new(),
                    state: MissingBlockState::Required,
                });

                self.shared.notify();
            }
        }

        true
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
//     missing_blocks[block_id].clients.get(client_id) == Some(ClientState::Offered)
//
// it must hold that
//
//     clients[client_id].contains(block_id)
//
// and vice-versa.
struct Inner {
    missing_blocks: HashMap<BlockId, MissingBlock>,
    clients: Slab<HashSet<BlockId>>,
    // Counts the number of completed blocks. Used to detect concurrent changes when requiring
    // blocks.
    complete_counter: u64,
}

struct MissingBlock {
    clients: HashMap<ClientId, ClientState>,
    state: MissingBlockState,
}

#[derive(Debug)]
enum MissingBlockState {
    Offered,
    Required,
    Accepted(ClientId),
}

enum ClientState {
    Offered,
    Cancelled,
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
    fn simple() {
        let tracker = BlockTracker::new();

        let client = tracker.client();

        // Initially no blocks are returned
        assert_eq!(client.try_accept(), None);

        // Offered but not required blocks are not returned
        let block0 = make_block();
        client.offer(block0.id);
        assert_eq!(client.try_accept(), None);

        // Required + offered blocks are returned...
        assert!(tracker.begin_require(block0.id).commit());
        assert_eq!(client.try_accept(), Some(block0.id));

        // ...but only once.
        assert_eq!(client.try_accept(), None);
    }

    #[test]
    fn required_unoffered() {
        let tracker = BlockTracker::new();
        let client = tracker.client();

        let block0 = make_block();
        let block1 = make_block();

        assert!(tracker.begin_require(block0.id).commit());
        assert!(tracker.begin_require(block1.id).commit());

        // Required + offered blocks are returned first...
        client.offer(block1.id);
        assert_eq!(client.try_accept(), Some(block1.id));

        // ...but required + non offered blocks are eventually returned as well
        assert_eq!(client.try_accept(), Some(block0.id));
    }

    #[test]
    fn required_unoffered_cancel() {
        let tracker = BlockTracker::new();
        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        assert!(tracker.begin_require(block.id).commit());
        assert_eq!(client0.try_accept(), Some(block.id));

        client0.cancel(&block.id);
        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn fallback_on_cancel_before_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        assert!(tracker.begin_require(block.id).commit());
        client0.offer(block.id);
        client1.offer(block.id);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn fallback_on_cancel_after_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        assert!(tracker.begin_require(block.id).commit());
        client0.offer(block.id);
        client1.offer(block.id);

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        client0.cancel(&block.id);

        assert_eq!(client0.try_accept(), None);
        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn fallback_on_client_drop_after_require_before_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        assert!(tracker.begin_require(block.id).commit());

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn fallback_on_client_drop_after_require_after_accept() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        assert!(tracker.begin_require(block.id).commit());

        assert_eq!(client0.try_accept(), Some(block.id));
        assert_eq!(client1.try_accept(), None);

        drop(client0);

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn fallback_on_client_drop_before_request() {
        let tracker = BlockTracker::new();

        let client0 = tracker.client();
        let client1 = tracker.client();

        let block = make_block();

        client0.offer(block.id);
        client1.offer(block.id);

        drop(client0);

        assert!(tracker.begin_require(block.id).commit());

        assert_eq!(client1.try_accept(), Some(block.id));
    }

    #[test]
    fn concurrent_require_and_complete() {
        let tracker = BlockTracker::new();
        let client = tracker.client();

        let block = make_block();
        client.offer(block.id);

        let require = tracker.begin_require(block.id);
        tracker.complete(&block.id);

        assert!(!require.commit());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn race() {
        let num_clients = 10;

        let tracker = BlockTracker::new();
        let clients: Vec<_> = (0..num_clients).map(|_| tracker.client()).collect();

        let block = make_block();

        assert!(tracker.begin_require(block.id).commit());

        for client in &clients {
            client.offer(block.id);
        }

        // Make sure all clients stay alive until we are done so that any accepted requests are not
        // released prematurely.
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

        let tracker = BlockTracker::new();
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
                Op::Require => {
                    assert!(tracker.begin_require(block_id).commit());
                }
                Op::Offer => {
                    client.offer(block_id);
                }
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
