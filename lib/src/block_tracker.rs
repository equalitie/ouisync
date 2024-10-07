use crate::{
    collections::{HashMap, HashSet},
    protocol::BlockId,
};
use std::{
    collections::hash_map::Entry,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{sync::mpsc, task};
use tracing::{Instrument, Span};

/// Tracks blocks that are offered for requesting from some peers and blocks that are required
/// locally. If a block is both, a notification is triggered to prompt us to request the block from
/// the offering peers.
///
/// Note the above applies only to read and write replicas. For blind replicas which blocks are
/// required is not known and so blocks are notified immediately once they've been offered.
#[derive(Clone)]
pub(crate) struct BlockTracker {
    command_tx: mpsc::UnboundedSender<Command>,
}

impl BlockTracker {
    pub fn new() -> Self {
        let (this, worker) = build();
        task::spawn(worker.run().instrument(Span::current()));
        this
    }

    /// Set the mode in which blocks are requested:
    ///
    /// - Lazy:   block is requested when it's both offered and required
    /// - Greedy: block is requested as soon as it's offered.
    ///
    /// Note: In `Greedy` mode calling `require` is unnecessary.
    pub fn set_request_mode(&self, mode: BlockRequestMode) {
        self.command_tx.send(Command::SetRequestMode { mode }).ok();
    }

    pub fn new_client(&self) -> (BlockTrackerClient, mpsc::UnboundedReceiver<BlockId>) {
        let (block_tx, block_rx) = mpsc::unbounded_channel();
        let client_id = ClientId::next();

        self.command_tx
            .send(Command::InsertClient {
                client_id,
                block_tx,
            })
            .ok();

        (
            BlockTrackerClient {
                client_id,
                command_tx: self.command_tx.clone(),
            },
            block_rx,
        )
    }

    pub fn require(&self, block_id: BlockId) {
        self.command_tx
            .send(Command::InsertRequired { block_id })
            .ok();
    }

    pub fn clear_required(&self) {
        self.command_tx.send(Command::ClearRequired).ok();
    }
}

pub(crate) struct BlockTrackerClient {
    client_id: ClientId,
    command_tx: mpsc::UnboundedSender<Command>,
}

impl BlockTrackerClient {
    /// Marks block as offered by this peer.
    pub fn offer(&self, block_id: BlockId, state: BlockOfferState) {
        self.command_tx
            .send(Command::InsertOffer {
                client_id: self.client_id,
                block_id,
                state,
            })
            .ok();
    }

    /// Marks a block offer as approved. Do this for a block offered (see [Self::offer]) previously
    /// with a `Complete` or `Incomplete` root node state after that root node has become
    /// `Approved`.
    ///
    /// Note: this marks all offers for the give block as approved, not just the ones from this
    /// peer.
    pub fn approve(&self, block_id: BlockId) {
        self.command_tx
            .send(Command::ApproveOffers { block_id })
            .ok();
    }
}

impl Drop for BlockTrackerClient {
    fn drop(&mut self) {
        self.command_tx
            .send(Command::RemoveClient {
                client_id: self.client_id,
            })
            .ok();
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum BlockRequestMode {
    // Request only required blocks
    Lazy,
    // Request all blocks
    Greedy,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BlockOfferState {
    Pending,
    Approved,
}

fn build() -> (BlockTracker, Worker) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    (BlockTracker { command_tx }, Worker::new(command_rx))
}

struct Worker {
    clients: HashMap<ClientId, ClientState>,
    required_blocks: HashSet<BlockId>,
    request_mode: BlockRequestMode,
    command_rx: mpsc::UnboundedReceiver<Command>,
}

impl Worker {
    fn new(command_rx: mpsc::UnboundedReceiver<Command>) -> Self {
        Self {
            clients: HashMap::default(),
            required_blocks: HashSet::default(),
            request_mode: BlockRequestMode::Greedy,
            command_rx,
        }
    }

    async fn run(mut self) {
        while let Some(command) = self.command_rx.recv().await {
            self.handle_command(command);
        }
    }

    /// Process all currently queued commands.
    #[cfg(test)]
    pub fn step(&mut self) {
        while let Ok(command) = self.command_rx.try_recv() {
            self.handle_command(command);
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::InsertClient {
                client_id,
                block_tx,
            } => self.insert_client(client_id, block_tx),
            Command::RemoveClient { client_id } => self.remove_client(client_id),
            Command::InsertOffer {
                client_id,
                block_id,
                state,
            } => self.insert_offer(client_id, block_id, state),
            Command::ApproveOffers { block_id } => self.handle_approve_offers(block_id),
            Command::SetRequestMode { mode } => self.set_request_mode(mode),
            Command::InsertRequired { block_id } => self.insert_required(block_id),
            Command::ClearRequired => self.clear_required(),
        }
    }

    fn insert_client(&mut self, client_id: ClientId, block_tx: mpsc::UnboundedSender<BlockId>) {
        self.clients.insert(client_id, ClientState::new(block_tx));
    }

    fn remove_client(&mut self, client_id: ClientId) {
        self.clients.remove(&client_id);
    }

    fn set_request_mode(&mut self, mode: BlockRequestMode) {
        self.request_mode = mode;

        match mode {
            BlockRequestMode::Greedy => {
                for client_state in self.clients.values_mut() {
                    client_state.accept_all_approved();
                }

                self.required_blocks.clear();
            }
            BlockRequestMode::Lazy => (),
        }
    }

    fn insert_required(&mut self, block_id: BlockId) {
        if !self.required_blocks.insert(block_id) {
            // already required
            return;
        }

        match self.request_mode {
            BlockRequestMode::Greedy => {
                for client_state in self.clients.values_mut() {
                    client_state.accept(block_id);
                }
            }
            BlockRequestMode::Lazy => {
                for client_state in self.clients.values_mut() {
                    client_state.accept_if_approved(block_id);
                }
            }
        }
    }

    fn clear_required(&mut self) {
        self.required_blocks.clear();
    }

    fn insert_offer(&mut self, client_id: ClientId, block_id: BlockId, new_state: BlockOfferState) {
        let Some(client_state) = self.clients.get_mut(&client_id) else {
            return;
        };

        match client_state.offers.entry(block_id) {
            Entry::Occupied(mut entry) => match (entry.get(), new_state) {
                (BlockOfferState::Pending, BlockOfferState::Approved) => {
                    if self.request_mode == BlockRequestMode::Greedy
                        || self.required_blocks.contains(&block_id)
                    {
                        entry.remove();
                        client_state.block_tx.send(block_id).ok();
                    } else {
                        entry.insert(BlockOfferState::Approved);
                    }
                }
                (BlockOfferState::Pending, BlockOfferState::Pending)
                | (BlockOfferState::Approved, BlockOfferState::Pending)
                | (BlockOfferState::Approved, BlockOfferState::Approved) => (),
            },
            Entry::Vacant(entry) => match new_state {
                BlockOfferState::Pending => {
                    entry.insert(BlockOfferState::Pending);
                }
                BlockOfferState::Approved => {
                    if self.request_mode == BlockRequestMode::Greedy
                        || self.required_blocks.contains(&block_id)
                    {
                        client_state.block_tx.send(block_id).ok();
                    } else {
                        entry.insert(BlockOfferState::Approved);
                    }
                }
            },
        }
    }

    fn handle_approve_offers(&mut self, block_id: BlockId) {
        let accept = matches!(self.request_mode, BlockRequestMode::Greedy)
            || self.required_blocks.contains(&block_id);

        for client_state in self.clients.values_mut() {
            if accept {
                client_state.accept(block_id);
            } else {
                client_state.approve(block_id);
            }
        }
    }
}

struct ClientState {
    offers: HashMap<BlockId, BlockOfferState>,
    block_tx: mpsc::UnboundedSender<BlockId>,
}

impl ClientState {
    fn new(block_tx: mpsc::UnboundedSender<BlockId>) -> Self {
        Self {
            offers: HashMap::default(),
            block_tx,
        }
    }

    fn accept(&mut self, block_id: BlockId) {
        if self.offers.remove(&block_id).is_some() {
            self.block_tx.send(block_id).ok();
        }
    }

    fn accept_if_approved(&mut self, block_id: BlockId) {
        let Entry::Occupied(entry) = self.offers.entry(block_id) else {
            return;
        };

        match entry.get() {
            BlockOfferState::Approved => {
                entry.remove();
                self.block_tx.send(block_id).ok();
            }
            BlockOfferState::Pending => (),
        }
    }

    fn accept_all_approved(&mut self) {
        self.offers.retain(|block_id, state| match state {
            BlockOfferState::Approved => {
                self.block_tx.send(*block_id).ok();
                false
            }
            BlockOfferState::Pending => true,
        });
    }

    fn approve(&mut self, block_id: BlockId) {
        if let Some(state) = self.offers.get_mut(&block_id) {
            *state = BlockOfferState::Approved;
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct ClientId(usize);

impl ClientId {
    fn next() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

enum Command {
    InsertClient {
        client_id: ClientId,
        block_tx: mpsc::UnboundedSender<BlockId>,
    },
    RemoveClient {
        client_id: ClientId,
    },
    InsertOffer {
        client_id: ClientId,
        block_id: BlockId,
        state: BlockOfferState,
    },
    ApproveOffers {
        block_id: BlockId,
    },
    SetRequestMode {
        mode: BlockRequestMode,
    },
    InsertRequired {
        block_id: BlockId,
    },
    ClearRequired,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::error::TryRecvError;

    #[test]
    fn greedy_mode_offer_pending_then_approve() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        // Note `Greedy` is the default mode

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Pending);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn greedy_mode_offer_approved() {
        let (tracker, mut worker) = build();

        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
    }

    #[test]
    fn greedy_mode_approve_non_existing_offer() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_offer_pending_then_approve_then_require() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Pending);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_offer_pending_then_require_then_approve() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Pending);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_require_then_offer_pending_then_approve() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Pending);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_offer_approved_then_require() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_require_then_offer_approved() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_approve_non_existing_offer() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        tracker.require(block_id);
        client.approve(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lazy_mode_offer_then_drop_client_then_require() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        worker.step();

        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        drop(client);
        tracker.require(block_id);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn switch_lazy_mode_to_greedy_mode() {
        let (tracker, mut worker) = build();
        let block_id_0 = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();
        let block_id_1 = BlockId::try_from([1; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        client.offer(block_id_0, BlockOfferState::Pending);
        client.offer(block_id_1, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));

        tracker.set_request_mode(BlockRequestMode::Greedy);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id_1));
        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn approve_affects_all_clients() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        let (client_a, mut block_rx_a) = tracker.new_client();
        let (client_b, mut block_rx_b) = tracker.new_client();

        client_a.offer(block_id, BlockOfferState::Pending);
        client_b.offer(block_id, BlockOfferState::Pending);

        worker.step();

        assert_eq!(block_rx_a.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(block_rx_b.try_recv(), Err(TryRecvError::Empty));

        client_a.approve(block_id);
        worker.step();

        assert_eq!(block_rx_a.try_recv(), Ok(block_id));
        assert_eq!(block_rx_b.try_recv(), Ok(block_id));
    }

    #[test]
    fn clear_required() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        tracker.set_request_mode(BlockRequestMode::Lazy);
        tracker.require(block_id);
        tracker.clear_required();
        worker.step();

        let (client, mut block_rx) = tracker.new_client();
        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn repeated_offer() {
        let (tracker, mut worker) = build();
        let block_id = BlockId::try_from([0; BlockId::SIZE].as_ref()).unwrap();

        let (client, mut block_rx) = tracker.new_client();
        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));

        client.offer(block_id, BlockOfferState::Approved);
        worker.step();

        assert_eq!(block_rx.try_recv(), Ok(block_id));
    }
}
