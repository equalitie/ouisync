use self::utils::{unlock, Command, Counter};
use super::Shared;
use crate::{
    blob::{BlobId, BlockIds},
    branch::Branch,
    directory::{DirectoryFallback, DirectoryLocking},
    error::{Error, Result},
    event::{self, Event, EventScope, Lagged, Payload},
    joint_directory::{JointDirectory, JointEntryRef, MissingVersionStrategy},
    store, versioned,
};
use async_recursion::async_recursion;
use futures_util::{stream, StreamExt};
use std::{future, sync::Arc};
use tokio::select;

#[cfg(test)]
mod tests;

/// Background worker to perform various jobs on the repository:
/// - merge remote branches into the local one
/// - remove outdated branches and snapshots
/// - remove unreachable blocks
/// - find missing blocks
pub(super) async fn run(shared: Arc<Shared>) {
    let event_scope = EventScope::new();
    let prune_counter = Counter::new();

    let local_branch = shared
        .local_branch()
        .ok()
        .filter(|branch| branch.keys().write().is_some())
        .map(|branch| branch.with_event_scope(event_scope));

    // Maintain (merge, prune and trash)
    let maintain = async {
        let (unlock_tx, unlock_rx) = unlock::channel();

        // - Ignore events from the same scope to prevent infinite loop
        // - On `BranchChanged` interrupt and restart the current job to avoid unnecessary work on
        //   potentially outdated branches.
        // - On any other event (including `Lagged`), let the current job run to completion and
        //   then restart it.
        let events =
            event::into_stream(shared.vault.event_tx.subscribe()).filter_map(move |event| {
                future::ready(match event {
                    Ok(Event { scope, .. }) if scope == event_scope => None,
                    Ok(Event {
                        payload: Payload::SnapshotApproved(_),
                        ..
                    }) => Some(Command::Interrupt),
                    Ok(Event {
                        payload: Payload::BlockReceived { .. },
                        ..
                    })
                    | Err(Lagged) => Some(Command::Wait),
                    Ok(Event {
                        payload: Payload::SnapshotRejected(_) | Payload::MaintenanceCompleted,
                        ..
                    }) => None,
                })
            });

        let unlocks = stream::unfold(unlock_rx, |mut rx| async move {
            if rx.recv().await {
                tracing::trace!("lock released");
                Some((Command::Wait, rx))
            } else {
                None
            }
        });

        let commands = stream::select(events, unlocks);

        utils::run(
            || maintain(&shared, local_branch.as_ref(), &unlock_tx, &prune_counter),
            commands,
        )
        .await;
    };

    // Scan
    let scan = async {
        // - On `BranchChanged` from outside of this scope restart the current job to avoid
        //   unnecessary traversal of potentially outdated branches.
        // - On `BranchChanged` from this scope, let the current job run to completion and then
        //   restart it. This is because such event can only come from `merge` which does not
        //   change the set of missing and required blocks.
        // - On any other event (including `Lagged`), let the current job run to completion and
        //   then restart it.
        let commands =
            event::into_stream(shared.vault.event_tx.subscribe()).filter_map(move |event| {
                future::ready(match event {
                    Ok(Event {
                        payload: Payload::SnapshotApproved(_),
                        scope,
                    }) if scope != event_scope => Some(Command::Interrupt),
                    Ok(Event {
                        payload: Payload::SnapshotApproved(_) | Payload::BlockReceived { .. },
                        ..
                    })
                    | Err(Lagged) => Some(Command::Wait),
                    Ok(Event {
                        payload: Payload::SnapshotRejected(_) | Payload::MaintenanceCompleted,
                        ..
                    }) => None,
                })
            });

        utils::run(|| scan(&shared, &prune_counter), commands).await;
    };

    // Run them in parallel so missing blocks are found as soon as possible
    select! {
        _ = maintain => (),
        _ = scan => (),
    }
}

async fn maintain(
    shared: &Shared,
    local_branch: Option<&Branch>,
    unlock_tx: &unlock::Sender,
    prune_counter: &Counter,
) {
    let mut success = true;

    // Merge branches
    if let Some(local_branch) = local_branch {
        let job_success = shared
            .vault
            .monitor
            .merge_job
            .run(merge::run(shared, local_branch))
            .await;
        success = success && job_success;
    }

    // Prune outdated branches and snapshots
    let job_success = shared
        .vault
        .monitor
        .prune_job
        .run(prune::run(shared, unlock_tx, prune_counter))
        .await;
    success = success && job_success;

    // Collect unreachable blocks
    if shared.credentials.read().unwrap().secrets.can_read() {
        let job_success = shared
            .vault
            .monitor
            .trash_job
            .run(trash::run(shared, local_branch, unlock_tx))
            .await;
        success = success && job_success;
    }

    if success {
        shared.vault.event_tx.send(Payload::MaintenanceCompleted);
    }
}

async fn scan(shared: &Shared, prune_counter: &Counter) {
    // Find missing blocks
    shared
        .vault
        .monitor
        .scan_job
        .run(scan::run(shared, prune_counter))
        .await;
}

/// Find missing blocks and mark them as required.
mod scan {
    use super::*;
    use tracing::instrument;

    pub(super) async fn run(shared: &Shared, prune_counter: &Counter) -> Result<()> {
        loop {
            let prune_count_before = prune_counter.get();

            match run_once(shared).await {
                Ok(()) => return Ok(()),
                // `BranchNotFound` and `LocatorNotFound` might be caused by a branch being pruned
                // concurrently as it's being scanned. Check the prune counter to confirm the prune
                // happened and if so, restart the scan.
                Err(Error::Store(store::Error::BranchNotFound | store::Error::LocatorNotFound))
                    if prune_counter.get() != prune_count_before =>
                {
                    continue
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn run_once(shared: &Shared) -> Result<()> {
        let branches = shared.load_branches().await?;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            let report_error = |error| {
                tracing::trace!(branch_id = ?branch.id(), ?error, "Failed to open root directory");
                error
            };

            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await
                .map_err(report_error)
            {
                Ok(dir) => {
                    versions.push(dir);
                }
                Err(Error::Store(store::Error::BlockNotFound)) => {
                    require_missing_blocks(shared, &branch, BlobId::ROOT).await?;
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
                        entry.inner().branch(),
                        *entry.inner().blob_id(),
                    )
                    .await?;
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        require_missing_blocks(shared, version.branch(), *version.blob_id())
                            .await?;
                    }

                    match entry
                        .open_with(MissingVersionStrategy::Fail, DirectoryFallback::Disabled)
                        .await
                    {
                        Ok(dir) => subdirs.push(dir),
                        Err(error) => {
                            // Continue processing the remaining entries
                            tracing::trace!(
                                entry = entry.name(),
                                ?error,
                                "Failed to open directory"
                            );
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

    #[instrument(skip(shared, branch), fields(branch_id = ?branch.id()))]
    async fn require_missing_blocks(
        shared: &Shared,
        branch: &Branch,
        blob_id: BlobId,
    ) -> Result<()> {
        let mut blob_block_ids =
            BlockIds::open(branch.clone(), blob_id)
                .await
                .map_err(|error| {
                    tracing::trace!(?error, "open failed");
                    error
                })?;
        let mut block_number = 0;
        let mut file_progress_cache_reset = false;
        let mut require_batch = shared.vault.block_tracker.require_batch();

        while let Some(block_id) = blob_block_ids.try_next().await.map_err(|error| {
            tracing::trace!(block_number, ?error, "try_next failed");
            error
        })? {
            if !shared
                .vault
                .store()
                .acquire_read()
                .await?
                .block_exists(&block_id)
                .await?
            {
                require_batch.add(block_id);

                if !file_progress_cache_reset {
                    file_progress_cache_reset = true;
                    branch.file_progress_cache().reset(&blob_id, block_number);
                }
            }

            block_number = block_number.saturating_add(1);
        }

        Ok(())
    }
}

/// Merge remote branches into the local one.
mod merge {
    use super::*;
    use crate::store;

    pub(super) async fn run(shared: &Shared, local_branch: &Branch) -> Result<()> {
        let branches: Vec<_> = shared.load_branches().await?;
        let mut roots = Vec::with_capacity(branches.len());

        for branch in &branches {
            // Use the `local_branch` instance to use the correct event scope.
            let branch = if branch.id() == local_branch.id() {
                local_branch
            } else {
                branch
            };

            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await
            {
                Ok(dir) => roots.push(dir),
                Err(Error::Store(store::Error::BlockNotFound)) => continue,
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
    use crate::{protocol::NodeState, versioned::PreferBranch};
    use futures_util::TryStreamExt;

    pub(super) async fn run(
        shared: &Shared,
        unlock_tx: &unlock::Sender,
        prune_counter: &Counter,
    ) -> Result<()> {
        let all: Vec<_> = shared
            .vault
            .store()
            .acquire_read()
            .await?
            .load_latest_preferred_root_nodes()
            .try_collect()
            .await?;

        let writer_id = shared.credentials.read().unwrap().writer_id;

        let (uptodate, outdated): (Vec<_>, Vec<_>) =
            versioned::partition(all, PreferBranch(Some(&writer_id)));

        // For the purpose of pruning, any approved snapshot is always considered more up-to-date
        // than any non-approved one even if the approved's version vector is happens-before the
        // non-approved one. This is because the non-approved ones are not visible to the users
        // until they become approved and there is no guarantee that they ever will (e.g., due to
        // peers disconnecting before receiving all nodes, etc...). For this reason, until there is
        // at least one branch with approved snapshot in `uptodate`, we consider all branches as
        // up-to-date.
        let (uptodate, outdated) = if uptodate
            .iter()
            .any(|node| node.summary.state == NodeState::Approved)
        {
            (uptodate, outdated)
        } else {
            (uptodate.into_iter().chain(outdated).collect(), Vec::new())
        };

        // Remove outdated branches
        for node in outdated {
            // Never remove local branch
            if node.proof.writer_id == writer_id {
                continue;
            }

            // Try to acquire a unique lock on the root directory of the branch. If any file or
            // directory from the branch is locked, the root will be locked as well and so this
            // acquire will fail, preventing us from pruning a branch that's still being used.
            let _lock = match shared
                .branch_shared
                .locker
                .branch(node.proof.writer_id)
                .try_unique(BlobId::ROOT)
            {
                Ok(lock) => lock,
                Err((notify, _)) => {
                    tracing::trace!(id = ?node.proof.writer_id, "outdated branch not removed - in use");
                    unlock_tx.send(notify).await;
                    continue;
                }
            };

            prune_counter.increment();

            let mut tx = shared.vault.store().begin_write().await?;
            tx.remove_branch(&node).await?;
            tx.commit().await?;

            tracing::trace!(
                branch_id = ?node.proof.writer_id,
                vv = ?node.proof.version_vector,
                hash = ?node.proof.hash,
                "outdated branch removed"
            );
        }

        // Remove outdated snapshots.
        for node in uptodate {
            shared
                .vault
                .store()
                .remove_outdated_snapshots(&node)
                .await?;
        }

        Ok(())
    }
}

/// Remove unreachable blocks
mod trash {
    use super::*;
    use crate::{
        crypto::sign::PublicKey,
        protocol::{BlockId, Bump},
        store::{Changeset, ReadTransaction, WriteTransaction},
    };
    use futures_util::TryStreamExt;
    use std::{
        collections::{BTreeSet, VecDeque},
        iter,
    };

    pub(super) async fn run(
        shared: &Shared,
        local_branch: Option<&Branch>,
        unlock_tx: &unlock::Sender,
    ) -> Result<()> {
        // Perform the scan in multiple passes, to avoid loading too many block ids into memory.
        const UNREACHABLE_BLOCKS_PAGE_SIZE: u32 = 1_000_000;

        let mut unreachable_block_ids_page =
            shared.vault.store().block_ids(UNREACHABLE_BLOCKS_PAGE_SIZE);

        loop {
            let mut unreachable_block_ids = unreachable_block_ids_page.next().await?;
            if unreachable_block_ids.is_empty() {
                break;
            }

            exclude_locked_blocks(shared, &mut unreachable_block_ids, unlock_tx).await?;

            traverse_root_in_all_branches(shared, local_branch, &mut unreachable_block_ids).await?;

            // If `merge` started but didn't complete (e.g., due to missing blocks), some of the
            // entries in the local branch might be outdated. We can't garbage collect their
            // blocks yet because they might still be needed in future `merge` (e.g., when those
            // missing blocks become available). Thus we traverse the local root again to exclude
            // all blocks that are reachable from it even if they belong to outdated entries.
            // When future `merge` completes, any such blocks will become unreachable and will be
            // collected during a subsequent `trash`.
            if let Some(local_branch) = local_branch {
                traverse_root_in_local_branch(local_branch, &mut unreachable_block_ids).await?;
            }

            remove_unreachable_blocks(shared, local_branch, unreachable_block_ids).await?;
        }

        Ok(())
    }

    async fn traverse_root_in_all_branches(
        shared: &Shared,
        local_branch: Option<&Branch>,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
    ) -> Result<()> {
        let local_branch_id = local_branch.map(Branch::id);
        let branches = shared.load_branches().await?;
        let mut versions = Vec::with_capacity(branches.len());

        for branch in branches {
            // Local blocks are be processed in `traverse_root_in_local_branch`, avoid processing
            // them twice.
            if Some(branch.id()) != local_branch_id {
                exclude_reachable_blocks(branch.clone(), BlobId::ROOT, unreachable_block_ids)
                    .await?;
            }

            // TODO: enable fallback so fallback blocks are not collected
            let dir = branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await?;
            versions.push(dir);
        }

        let dir = JointDirectory::new(local_branch.cloned(), versions);

        traverse(dir, local_branch_id, unreachable_block_ids).await
    }

    async fn traverse_root_in_local_branch(
        local_branch: &Branch,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
    ) -> Result<()> {
        exclude_reachable_blocks(local_branch.clone(), BlobId::ROOT, unreachable_block_ids).await?;

        let dir = local_branch
            .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
            .await?;
        let dir = JointDirectory::new(Some(local_branch.clone()), iter::once(dir));

        traverse(dir, None, unreachable_block_ids).await
    }

    async fn traverse(
        dir: JointDirectory,
        skip_branch_id: Option<&PublicKey>,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
    ) -> Result<()> {
        let mut queue: VecDeque<_> = iter::once(dir).collect();

        while let Some(dir) = queue.pop_back() {
            for entry in dir.entries() {
                match entry {
                    JointEntryRef::File(entry) => {
                        if Some(entry.branch().id()) == skip_branch_id {
                            continue;
                        }

                        exclude_reachable_blocks(
                            entry.inner().branch().clone(),
                            *entry.inner().blob_id(),
                            unreachable_block_ids,
                        )
                        .await?;
                    }
                    JointEntryRef::Directory(entry) => {
                        for version in entry.versions() {
                            if Some(version.branch().id()) == skip_branch_id {
                                continue;
                            }

                            exclude_reachable_blocks(
                                version.branch().clone(),
                                *version.blob_id(),
                                unreachable_block_ids,
                            )
                            .await?;
                        }

                        let dir = entry
                            .open_with(MissingVersionStrategy::Fail, DirectoryFallback::Disabled)
                            .await?;
                        queue.push_front(dir);
                    }
                }
            }
        }

        Ok(())
    }

    async fn exclude_reachable_blocks(
        branch: Branch,
        blob_id: BlobId,
        unreachable_block_ids: &mut BTreeSet<BlockId>,
    ) -> Result<()> {
        let mut blob_block_ids = BlockIds::open(branch, blob_id).await?;

        while let Some(block_id) = blob_block_ids.try_next().await? {
            unreachable_block_ids.remove(&block_id);
        }

        Ok(())
    }

    /// Exclude blocks of locked blobs from the `unreachable_block_ids` set.
    async fn exclude_locked_blocks(
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
            let Ok(branch) = shared.get_branch(branch_id) else {
                continue;
            };

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

            let mut tx = shared.vault.store().begin_write().await?;

            total_count += batch.len();

            if let Some((local_branch, write_keys)) = &local_branch_and_write_keys {
                let mut changeset = Changeset::new();
                remove_local_nodes(&mut tx, &mut changeset, &batch).await?;
                changeset.bump(Bump::increment(*local_branch.id()));
                changeset
                    .apply(&mut tx, local_branch.id(), write_keys)
                    .await?;
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
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        block_ids: &[BlockId],
    ) -> Result<()> {
        for block_id in block_ids {
            let locators: Vec<_> = tx.load_locators(block_id).try_collect().await?;

            for locator in locators {
                changeset.unlink_block(locator, Some(*block_id));
            }
        }

        Ok(())
    }

    async fn remove_blocks(tx: &mut WriteTransaction, block_ids: &[BlockId]) -> Result<()> {
        for block_id in block_ids {
            tx.remove_block(block_id).await?;
            tracing::trace!(?block_id, "unreachable block removed");
        }

        Ok(())
    }
}

mod utils {
    use futures_util::{Stream, StreamExt};
    use std::{
        future::Future,
        pin::pin,
        sync::atomic::{AtomicU64, Ordering},
    };
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

    #[derive(Default)]
    pub(super) struct Counter(AtomicU64);

    impl Counter {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn get(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }

        pub fn increment(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
