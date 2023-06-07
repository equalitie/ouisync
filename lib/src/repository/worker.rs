use self::utils::{unlock, Command};
use super::Shared;
use crate::{
    blob::BlockIds,
    blob_id::BlobId,
    branch::Branch,
    directory::{DirectoryFallback, DirectoryLocking},
    error::{Error, Result},
    event::{Event, EventScope, IgnoreScopeReceiver, Payload},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    versioned,
};
use async_recursion::async_recursion;
use futures_util::{stream, Stream};
use std::sync::Arc;
use tokio::sync::broadcast::{self, error::RecvError};

/// Background worker to perform various jobs on the repository:
/// - merge remote branches into the local one
/// - remove outdated branches and snapshots
/// - remove unreachable blocks
/// - find missing blocks
pub(super) async fn run(shared: Arc<Shared>, local_branch: Option<Branch>) {
    let event_scope = EventScope::new();
    let local_branch = local_branch.map(|branch| branch.with_event_scope(event_scope));

    let event_rx = shared.store.index.subscribe();
    let (unlock_tx, unlock_rx) = unlock::channel();
    let commands = stream::select(events(event_rx, event_scope), unlocks(unlock_rx));

    utils::run(
        || maintain(&shared, local_branch.as_ref(), &unlock_tx),
        commands,
    )
    .await;
}

fn events(rx: broadcast::Receiver<Event>, scope: EventScope) -> impl Stream<Item = Command> {
    let rx = IgnoreScopeReceiver::new(rx, scope);

    stream::unfold(rx, |mut rx| async move {
        let event = rx.recv().await;

        match &event {
            Ok(payload) => tracing::trace!(?payload, "event received"),
            Err(RecvError::Lagged(_)) => tracing::trace!("event receiver lagged"),
            Err(RecvError::Closed) => tracing::trace!("event receiver closed"),
        }

        match event {
            Ok(Payload::BranchChanged(_)) => {
                // On `BranchChanged`, interrupt the current job and
                // immediately start a new one.
                Some((Command::Interrupt, rx))
            }
            Ok(Payload::BlockReceived { .. }) | Err(RecvError::Lagged(_)) => {
                // On any other event, let the current job run to completion
                // and then start a new one.
                Some((Command::Wait, rx))
            }
            Err(RecvError::Closed) => None,
        }
    })
}

fn unlocks(rx: unlock::Receiver) -> impl Stream<Item = Command> {
    stream::unfold(rx, |mut rx| async move {
        if rx.recv().await {
            tracing::trace!("lock released");
            Some((Command::Wait, rx))
        } else {
            None
        }
    })
}

async fn maintain(shared: &Shared, local_branch: Option<&Branch>, unlock_tx: &unlock::Sender) {
    // Find missing blocks
    shared.store.monitor.scan_job.run(scan::run(shared)).await;

    // Merge branches
    if let Some(local_branch) = local_branch {
        shared
            .store
            .monitor
            .merge_job
            .run(merge::run(shared, local_branch))
            .await;
    }

    // Prune outdated branches and snapshots
    shared
        .store
        .monitor
        .prune_job
        .run(prune::run(shared, unlock_tx))
        .await;

    // Collect unreachable blocks
    if shared.secrets.can_read() {
        shared
            .store
            .monitor
            .trash_job
            .run(trash::run(shared, local_branch, unlock_tx))
            .await;
    }
}

/// Find missing blocks and mark them as required.
mod scan {
    use super::*;

    pub(super) async fn run(shared: &Shared) -> Result<()> {
        let branches = shared.load_branches().await?;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            require_missing_blocks(shared, branch.clone(), BlobId::ROOT).await?;

            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await.map_err(|error| {
                    tracing::warn!(branch_id = ?branch.id(), ?error, "failed to open root directory");
                    error
                })
            {
                Ok(dir) => versions.push(dir),
                Err(Error::EntryNotFound) => {
                    // `EntryNotFound` here just means this is a newly created branch with no
                    // content yet. It is safe to ignore it.
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        traverse(shared, JointDirectory::new(None, versions)).await
    }

    #[async_recursion]
    async fn traverse(shared: &Shared, dir: JointDirectory) -> Result<()> {
        let mut subdirs = Vec::new();

        for entry in dir.entries() {
            match entry {
                JointEntryRef::File(entry) => {
                    require_missing_blocks(
                        shared,
                        entry.inner().branch().clone(),
                        *entry.inner().blob_id(),
                    )
                    .await?;
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        require_missing_blocks(
                            shared,
                            version.branch().clone(),
                            *version.blob_id(),
                        )
                        .await?;
                    }

                    match entry
                        .open_with(MissingVersionStrategy::Fail, DirectoryFallback::Disabled)
                        .await
                    {
                        Ok(dir) => subdirs.push(dir),
                        Err(error) => {
                            // Continue processing the remaining entries
                            tracing::warn!(name = entry.name(), ?error, "failed to open directory");
                        }
                    }
                }
            }
        }

        for dir in subdirs {
            traverse(shared, dir).await?;
        }

        Ok(())
    }

    async fn require_missing_blocks(
        shared: &Shared,
        branch: Branch,
        blob_id: BlobId,
    ) -> Result<()> {
        let mut blob_block_ids = BlockIds::open(branch, blob_id).await?;

        while let Some(block_id) = blob_block_ids.try_next().await? {
            shared.store.require_missing_block(block_id).await?;
        }

        Ok(())
    }
}

/// Merge remote branches into the local one.
mod merge {
    use super::*;

    pub(super) async fn run(shared: &Shared, local_branch: &Branch) -> Result<()> {
        let branches: Vec<_> = shared.load_branches().await?;
        let mut roots = Vec::with_capacity(branches.len());

        for branch in branches {
            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await
            {
                Ok(dir) => roots.push(dir),
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        match JointDirectory::new(Some(local_branch.clone()), roots)
            .merge()
            .await
        {
            Ok(_) | Err(Error::AmbiguousEntry) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// Remove outdated branches and snapshots.
mod prune {
    use super::*;
    use crate::{
        crypto::sign::PublicKey,
        index::{MultiBlockPresence, SnapshotData},
    };
    use std::cmp::Ordering;

    pub(super) async fn run(shared: &Shared, unlock_tx: &unlock::Sender) -> Result<()> {
        let all = shared.store.index.load_snapshots().await?;

        // When there are multiple branches with the same vv but different hash we need to preserve
        // them because we might need them to request missing blocks. But once the local branch has
        // all blocks we can prune them.
        let (uptodate, outdated): (Vec<_>, Vec<_>) =
            versioned::partition(all, Tiebreaker(&shared.this_writer_id));

        // Remove outdated branches
        for snapshot in outdated {
            // Never remove local branch
            if snapshot.branch_id() == &shared.this_writer_id {
                continue;
            }

            // Try to acquire a unique lock on the root directory of the branch. If any file or
            // directory from the branch is locked, the root will be locked as well and so this
            // acquire will fail, preventing us from pruning a branch that's still being used.
            let _lock = match shared
                .branch_shared
                .locker
                .branch(*snapshot.branch_id())
                .try_unique(BlobId::ROOT)
            {
                Ok(lock) => lock,
                Err((notify, _)) => {
                    tracing::trace!(id = ?snapshot.branch_id(), "outdated branch not removed - in use");
                    unlock_tx.send(notify).await;
                    continue;
                }
            };

            let mut tx = shared.store.db().begin_write().await?;
            snapshot.remove_all_older(&mut tx).await?;
            snapshot.remove(&mut tx).await?;
            tx.commit().await?;

            tracing::trace!(
                branch_id = ?snapshot.branch_id(),
                vv = ?snapshot.version_vector(),
                hash = ?snapshot.root_hash(),
                "outdated branch removed"
            );
        }

        // Remove outdated snapshots.
        for snapshot in uptodate {
            snapshot.prune(shared.store.db()).await?;
        }

        Ok(())
    }

    // If one of the snapshots is local and has all blocks discard the other one, otherwise keep
    // both.
    struct Tiebreaker<'a>(&'a PublicKey);

    impl versioned::Tiebreaker<SnapshotData> for Tiebreaker<'_> {
        fn break_tie(&self, lhs: &SnapshotData, rhs: &SnapshotData) -> Ordering {
            match (lhs.branch_id() == self.0, rhs.branch_id() == self.0) {
                (true, false) if lhs.block_presence() == &MultiBlockPresence::Full => {
                    Ordering::Greater
                }
                (false, true) if rhs.block_presence() == &MultiBlockPresence::Full => {
                    Ordering::Less
                }
                _ => Ordering::Equal,
            }
        }
    }
}

/// Remove unreachable blocks
mod trash {
    use super::*;
    use crate::{
        block::{self, BlockId},
        crypto::sign::Keypair,
        db,
        index::{self, LeafNode, SnapshotData, UpdateSummaryReason},
    };
    use futures_util::TryStreamExt;
    use std::collections::BTreeSet;
    use tracing::Instrument;

    pub(super) async fn run(
        shared: &Shared,
        local_branch: Option<&Branch>,
        unlock_tx: &unlock::Sender,
    ) -> Result<()> {
        // Perform the scan in multiple passes, to avoid loading too many block ids into memory.
        // The first pass is used both for requiring missing blocks and collecting unreachable
        // blocks. The subsequent passes (if any) for collecting only.
        const UNREACHABLE_BLOCKS_PAGE_SIZE: u32 = 1_000_000;

        let mut unreachable_block_ids_page = shared.store.block_ids(UNREACHABLE_BLOCKS_PAGE_SIZE);

        loop {
            let mut unreachable_block_ids = unreachable_block_ids_page.next().await?;
            if unreachable_block_ids.is_empty() {
                break;
            }

            process_locked_blocks(shared, &mut unreachable_block_ids, unlock_tx).await?;

            traverse_root(shared, local_branch, &mut unreachable_block_ids).await?;
            remove_unreachable_blocks(shared, local_branch, unreachable_block_ids).await?;
        }

        Ok(())
    }

    async fn traverse_root(
        shared: &Shared,
        local_branch: Option<&Branch>,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
    ) -> Result<()> {
        let branches = shared.load_branches().await?;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            process_reachable_blocks(unreachable_block_ids, branch.clone(), BlobId::ROOT).await?;

            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await.map_err(|error| {
                    tracing::warn!(branch_id = ?branch.id(), ?error, "failed to open root directory");
                    error
                })
            {
                Ok(dir) => versions.push(dir),
                Err(Error::EntryNotFound) => {
                    // `EntryNotFound` here just means this is a newly created branch with no
                    // content yet. It is safe to ignore it.
                    continue;
                }
                Err(error) => return Err(error),
            }
        }

        traverse(
            unreachable_block_ids,
            JointDirectory::new(local_branch.cloned(), versions),
        )
        .await
    }

    #[async_recursion]
    async fn traverse(
        unreachable_block_ids: &mut BTreeSet<BlockId>,
        dir: JointDirectory,
    ) -> Result<()> {
        let mut subdirs = Vec::new();

        for entry in dir.entries() {
            match entry {
                JointEntryRef::File(entry) => {
                    process_reachable_blocks(
                        unreachable_block_ids,
                        entry.inner().branch().clone(),
                        *entry.inner().blob_id(),
                    )
                    .await?;
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        process_reachable_blocks(
                            unreachable_block_ids,
                            version.branch().clone(),
                            *version.blob_id(),
                        )
                        .await?;
                    }

                    let dir = entry
                        .open_with(MissingVersionStrategy::Fail, DirectoryFallback::Disabled)
                        .await?;
                    subdirs.push(dir);
                }
            }
        }

        for dir in subdirs {
            traverse(unreachable_block_ids, dir).await?;
        }

        Ok(())
    }

    async fn process_reachable_blocks(
        unreachable_block_ids: &mut BTreeSet<BlockId>,
        branch: Branch,
        blob_id: BlobId,
    ) -> Result<()> {
        let mut blob_block_ids = BlockIds::open(branch, blob_id).await?;

        while let Some(block_id) = blob_block_ids.try_next().await? {
            unreachable_block_ids.remove(&block_id);
        }

        Ok(())
    }

    /// Remove blocks of locked blobs from the `unreachable_block_ids` set.
    async fn process_locked_blocks(
        shared: &Shared,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
        unlock_tx: &unlock::Sender,
    ) -> Result<()> {
        // This can sometimes include pruned branches. It happens when a branch is first loaded,
        // then pruned, then in an attempt to open the root directory, it's read lock is acquired
        // but before the open fails and the lock is dropped, we already return the lock here.
        // When this happens then the subsequent `BlockIds::open` might fail with `EntryNotFound`
        // but we ignore it because it's harmless.
        let locks = shared.branch_shared.locker.all();
        if locks.is_empty() {
            return Ok(());
        }

        for (branch_id, locks) in locks {
            let Ok(branch) = shared.get_branch(branch_id) else { continue };

            for (blob_id, notify) in locks {
                let mut blob_block_ids = match BlockIds::open(branch.clone(), blob_id).await {
                    Ok(block_ids) => block_ids,
                    Err(Error::EntryNotFound) => continue, // See the comment above.
                    Err(error) => return Err(error),
                };

                unlock_tx.send(notify).await;

                while let Some(block_id) = blob_block_ids.try_next().await? {
                    unreachable_block_ids.remove(&block_id);
                }
            }
        }

        Ok(())
    }

    async fn remove_unreachable_blocks(
        shared: &Shared,
        local_branch: Option<&Branch>,
        unreachable_block_ids: BTreeSet<BlockId>,
    ) -> Result<()> {
        // We need to delete the blocks and also mark them as missing (so they can be requested in
        // case they become needed again) in their corresponding leaf nodes and then update the
        // summaries of the corresponding ancestor nodes. This is a complex and potentially
        // expensive operation which is why we do it a few blocks at a time.
        const BATCH_SIZE: usize = 32;

        let mut unreachable_block_ids = unreachable_block_ids.into_iter();
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut total_count = 0;

        let local_branch_and_write_keys = local_branch
            .as_ref()
            .and_then(|branch| branch.keys().write().map(|keys| (branch, keys)));

        loop {
            batch.clear();
            batch.extend(unreachable_block_ids.by_ref().take(BATCH_SIZE));

            if batch.is_empty() {
                break;
            }

            let mut tx = shared.store.db().begin_write().await?;

            total_count += batch.len();

            if let Some((local_branch, write_keys)) = &local_branch_and_write_keys {
                let mut snapshot = local_branch.data().load_snapshot(&mut tx).await?;
                remove_local_nodes(&mut tx, &mut snapshot, write_keys, &batch).await?;
            }

            remove_blocks(&mut tx, &batch).await?;

            if let Some((branch, _)) = local_branch_and_write_keys {
                // If we modified the local branch (by removing nodes from it), we need to notify,
                // to let other replicas know about the change. Using `commit_and_then` to handle
                // possible cancellation.
                let event_tx = branch.notify();
                tx.commit_and_then(move || event_tx.send()).await?
            } else {
                // Using regular `commit` here because if there is nothing to notify then we don't
                // care about cancellation.
                tx.commit().await?;
            }
        }

        if total_count > 0 {
            tracing::debug!("unreachable blocks removed: {}", total_count);
        }

        Ok(())
    }

    async fn remove_local_nodes(
        tx: &mut db::WriteTransaction,
        snapshot: &mut SnapshotData,
        write_keys: &Keypair,
        block_ids: &[BlockId],
    ) -> Result<()> {
        for block_id in block_ids {
            let locators: Vec<_> = LeafNode::load_locators(tx, block_id).try_collect().await?;
            let span = tracing::info_span!("remove_local_node", ?block_id);

            for locator in locators {
                match snapshot
                    .remove_block(tx, &locator, Some(block_id), write_keys)
                    .instrument(span.clone())
                    .await
                {
                    Ok(()) | Err(Error::EntryNotFound) => (),
                    Err(error) => return Err(error),
                }
            }
        }

        snapshot.remove_all_older(tx).await?;

        Ok(())
    }

    async fn remove_blocks(tx: &mut db::WriteTransaction, block_ids: &[BlockId]) -> Result<()> {
        for block_id in block_ids {
            tracing::trace!(?block_id, "unreachable block removed");

            block::remove(tx, block_id).await?;

            LeafNode::set_missing(tx, block_id).await?;

            let parent_hashes: Vec<_> = LeafNode::load_parent_hashes(tx, block_id)
                .try_collect()
                .await?;

            index::update_summaries(tx, parent_hashes, UpdateSummaryReason::BlockRemoved).await?;
        }

        Ok(())
    }
}

mod utils {
    use futures_util::{Stream, StreamExt};
    use std::{future::Future, pin::pin};
    use tokio::select;

    /// Control how the next job should run
    pub(super) enum Command {
        // Wait for the current job to finish before starting a new one
        Wait,
        // Interrupt the current job and start a new one immediatelly
        Interrupt,
    }

    /// Runs the given job in a loop based on commands received from the given command stream.
    pub(super) async fn run<JobFn, Job, Commands>(mut job_fn: JobFn, commands: Commands)
    where
        JobFn: FnMut() -> Job,
        Job: Future<Output = ()>,
        Commands: Stream<Item = Command>,
    {
        enum State {
            Working,
            Waiting,
            Terminated,
        }

        let mut state = State::Working;
        let mut commands = pin!(commands);

        loop {
            match state {
                State::Working => {
                    state = State::Waiting;

                    let work = job_fn();
                    let wait = async {
                        loop {
                            match commands.next().await {
                                Some(Command::Wait) => {
                                    state = State::Working;
                                }
                                Some(Command::Interrupt) => {
                                    state = State::Working;
                                    break;
                                }
                                None => {
                                    state = State::Terminated;
                                    break;
                                }
                            }
                        }
                    };

                    select! {
                        _ = work => (),
                        _ = wait => (),
                    }
                }
                State::Waiting => {
                    state = match commands.next().await {
                        Some(Command::Wait | Command::Interrupt) => State::Working,
                        None => State::Terminated,
                    }
                }
                State::Terminated => break,
            }
        }
    }

    /// Register and await unlock notifications.
    pub(super) mod unlock {
        use crate::sync::AwaitDrop;
        use futures_util::{stream::FuturesUnordered, StreamExt};
        use tokio::{select, sync::mpsc};

        pub(crate) struct Sender(mpsc::Sender<AwaitDrop>);

        impl Sender {
            pub(crate) async fn send(&self, notify: AwaitDrop) {
                self.0.send(notify).await.ok();
            }
        }

        pub(crate) struct Receiver {
            pending: FuturesUnordered<AwaitDrop>,
            rx: mpsc::Receiver<AwaitDrop>,
        }

        impl Receiver {
            pub(crate) async fn recv(&mut self) -> bool {
                loop {
                    select! {
                        _ = self.pending.next(), if !self.pending.is_empty() => {
                            return true;
                        }
                        notify = self.rx.recv() => {
                            if let Some(notify) = notify {
                                self.pending.push(notify);
                            } else {
                                return false;
                            }
                        }
                    }
                }
            }
        }

        pub(crate) fn channel() -> (Sender, Receiver) {
            let (tx, rx) = mpsc::channel(1);

            (
                Sender(tx),
                Receiver {
                    pending: FuturesUnordered::new(),
                    rx,
                },
            )
        }
    }
}
