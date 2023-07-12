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
use tokio::{
    select,
    sync::broadcast::{self, error::RecvError},
};

/// Background worker to perform various jobs on the repository:
/// - merge remote branches into the local one
/// - remove outdated branches and snapshots
/// - remove unreachable blocks
/// - find missing blocks
pub(super) async fn run(shared: Arc<Shared>, local_branch: Option<Branch>) {
    let event_scope = EventScope::new();

    // Maintain (merge, prune and trash)
    let maintain = async {
        let local_branch = local_branch.map(|branch| branch.with_event_scope(event_scope));
        let (unlock_tx, unlock_rx) = unlock::channel();
        let commands = stream::select(
            from_events(shared.vault.event_tx.subscribe(), event_scope),
            from_unlocks(unlock_rx),
        );

        utils::run(
            || maintain(&shared, local_branch.as_ref(), &unlock_tx),
            commands,
        )
        .await;
    };

    // Scan
    let scan = async {
        let commands = from_events(shared.vault.event_tx.subscribe(), event_scope);
        utils::run(|| scan(&shared), commands).await;
    };

    // Run them in parallel so missing blocks are found as soon as possible
    select! {
        _ = maintain => (),
        _ = scan => (),
    }
}

fn from_events(rx: broadcast::Receiver<Event>, scope: EventScope) -> impl Stream<Item = Command> {
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

fn from_unlocks(rx: unlock::Receiver) -> impl Stream<Item = Command> {
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
    // Merge branches
    if let Some(local_branch) = local_branch {
        shared
            .vault
            .monitor
            .merge_job
            .run(merge::run(shared, local_branch))
            .await;
    }

    // Prune outdated branches and snapshots
    shared
        .vault
        .monitor
        .prune_job
        .run(prune::run(shared, unlock_tx))
        .await;

    // Collect unreachable blocks
    if shared.secrets.can_read() {
        shared
            .vault
            .monitor
            .trash_job
            .run(trash::run(shared, local_branch, unlock_tx))
            .await;
    }
}

async fn scan(shared: &Shared) {
    // Find missing blocks
    shared.vault.monitor.scan_job.run(scan::run(shared)).await
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
                    tracing::trace!(branch_id = ?branch.id(), ?error, "failed to open root directory");
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
                            tracing::trace!(
                                name = entry.name(),
                                ?error,
                                "failed to open directory"
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

    async fn require_missing_blocks(
        shared: &Shared,
        branch: Branch,
        blob_id: BlobId,
    ) -> Result<()> {
        let mut blob_block_ids = BlockIds::open(branch.clone(), blob_id).await?;
        let mut block_number = 0;
        let mut file_progress_cache_reset = false;

        while let Some((block_id, presence)) = blob_block_ids.try_next().await? {
            if !presence.is_present() {
                shared.vault.block_tracker.require(block_id);

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

        for branch in branches {
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
    use futures_util::TryStreamExt;

    pub(super) async fn run(shared: &Shared, unlock_tx: &unlock::Sender) -> Result<()> {
        let all: Vec<_> = shared
            .vault
            .store()
            .acquire_read()
            .await?
            .load_root_nodes()
            .try_collect()
            .await?;

        // Currently it's possible that multiple branches have the same vv but different hash.
        // There is no good and simple way to pick one over the other so we currently keep all.
        // TODO: When https://github.com/equalitie/ouisync/issues/113 is fixed, change this to use
        // the `PreferBranch` tiebreaker using the local branch id.
        let (uptodate, outdated): (Vec<_>, Vec<_>) = versioned::partition(all, ());

        // Remove outdated branches
        for node in outdated {
            // Never remove local branch
            if node.proof.writer_id == shared.this_writer_id {
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
        block::BlockId,
        crypto::sign::{Keypair, PublicKey},
        store::{self, WriteTransaction},
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

        let mut unreachable_block_ids_page = shared.vault.block_ids(UNREACHABLE_BLOCKS_PAGE_SIZE);

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

            // TODO: enable fallback so fallback blocks are not collected
            match branch
                .open_root(DirectoryLocking::Disabled, DirectoryFallback::Disabled)
                .await.map_err(|error| {
                    tracing::trace!(branch_id = ?branch.id(), ?error, "failed to open root directory");
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

        while let Some((block_id, _)) = blob_block_ids.try_next().await? {
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

                while let Some((block_id, _)) = blob_block_ids.try_next().await? {
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
                remove_local_nodes(&mut tx, local_branch.id(), write_keys, &batch).await?;
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
        tx: &mut WriteTransaction,
        branch_id: &PublicKey,
        write_keys: &Keypair,
        block_ids: &[BlockId],
    ) -> Result<()> {
        for block_id in block_ids {
            let locators: Vec<_> = tx.load_locators(block_id).try_collect().await?;
            let span = tracing::info_span!("remove_local_node", ?block_id);

            for locator in locators {
                match tx
                    .unlink_block(branch_id, &locator, Some(block_id), write_keys)
                    .instrument(span.clone())
                    .await
                {
                    Ok(()) | Err(store::Error::LocatorNotFound) => (),
                    Err(error) => return Err(error.into()),
                }
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
