use super::{utils, Shared};
use crate::{
    branch::Branch,
    debug::instrument,
    error::{Error, Result},
    event::{EventScope, IgnoreScopeReceiver},
    joint_directory::JointDirectory,
};
use std::sync::Arc;
use tokio::{
    select,
    sync::{broadcast::error::RecvError, mpsc, oneshot},
};

/// Background worker to perform various jobs on the repository:
/// - merge remote branches into the local one
/// - remove outdated branches and snapshots
/// - remove unreachable blocks
/// - find missing blocks
pub(super) struct Worker {
    inner: Inner,
    command_rx: mpsc::Receiver<Command>,
    abort_rx: oneshot::Receiver<()>,
}

impl Worker {
    pub fn new(shared: Arc<Shared>, local_branch: Option<Branch>) -> (Self, WorkerHandle) {
        let (command_tx, command_rx) = mpsc::channel(1);
        let (abort_tx, abort_rx) = oneshot::channel();

        let inner = Inner {
            shared,
            local_branch,
            event_scope: EventScope::new(),
        };

        let worker = Self {
            inner,
            command_rx,
            abort_rx,
        };
        let handle = WorkerHandle {
            command_tx,
            _abort_tx: abort_tx,
        };

        (worker, handle)
    }

    pub async fn run(self) {
        select! {
            _ = self.inner.run(self.command_rx) => (),
            _ = self.abort_rx => (),
        }
    }
}

/// Handle to interact with the worker. Aborts the worker task when dropped.
pub(super) struct WorkerHandle {
    command_tx: mpsc::Sender<Command>,
    _abort_tx: oneshot::Sender<()>,
}

impl WorkerHandle {
    pub async fn merge(&self) -> Result<()> {
        self.oneshot(Command::Merge).await
    }

    pub async fn collect(&self) -> Result<()> {
        self.oneshot(Command::Collect).await
    }

    async fn oneshot<F>(&self, command_fn: F) -> Result<()>
    where
        F: FnOnce(oneshot::Sender<Result<()>>) -> Command,
    {
        let (result_tx, result_rx) = oneshot::channel();
        self.command_tx
            .send(command_fn(result_tx))
            .await
            .unwrap_or(());

        // When this returns error it means the worker has been terminated which we treat as
        // success, for simplicity.
        result_rx.await.unwrap_or(Ok(()))
    }
}

enum Command {
    Merge(oneshot::Sender<Result<()>>),
    Collect(oneshot::Sender<Result<()>>),
}

struct Inner {
    shared: Arc<Shared>,
    local_branch: Option<Branch>,
    event_scope: EventScope,
}

impl Inner {
    async fn run(self, mut command_rx: mpsc::Receiver<Command>) {
        let mut event_rx =
            IgnoreScopeReceiver::new(self.shared.store.index.subscribe(), self.event_scope);
        let mut wait = false;

        loop {
            // Wait for notification or command.
            if wait {
                select! {
                    event = event_rx.recv() => {
                        match event {
                            Ok(_) | Err(RecvError::Lagged(_)) => (),
                            Err(RecvError::Closed) => break,
                        }
                    }
                    command = command_rx.recv() => {
                        if let Some(command) = command {
                            self.handle_command(command).await;
                            continue;
                        } else {
                            break;
                        }
                    }
                }
            }

            // Run the merge job but interrupt and restart it when we receive another notification.
            select! {
                event = event_rx.recv() => {
                    match event {
                        Ok(_) | Err(RecvError::Lagged(_)) => {
                            wait = false;
                            continue
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
                result = self.merge() => result.unwrap_or(()),
            }

            // Run the remaining jobs without interruption
            self.prune().await.unwrap_or(());
            self.scan(scan::Mode::RequireAndCollect).await.unwrap_or(());

            wait = true;
        }
    }

    async fn handle_command(&self, command: Command) {
        match command {
            Command::Merge(result_tx) => result_tx.send(self.merge().await).unwrap_or(()),
            Command::Collect(result_tx) => result_tx.send(self.collect().await).unwrap_or(()),
        }
    }

    async fn merge(&self) -> Result<()> {
        let local_branch = if let Some(local_branch) = &self.local_branch {
            local_branch
        } else {
            return Ok(());
        };

        instrument(
            "merge",
            self.event_scope
                .apply(merge::run(&self.shared, local_branch)),
        )
        .await
    }

    async fn prune(&self) -> Result<()> {
        instrument("prune", self.event_scope.apply(prune::run(&self.shared))).await
    }

    async fn scan(&self, mode: scan::Mode) -> Result<()> {
        if !self.shared.secrets.can_read() {
            return Ok(());
        }

        instrument("scan", scan::run(&self.shared, mode)).await
    }

    async fn collect(&self) -> Result<()> {
        self.prune().await?;
        self.scan(scan::Mode::Collect).await?;
        Ok(())
    }
}

/// Merge remote branches into the local one.
mod merge {
    use super::*;

    pub(super) async fn run(shared: &Shared, local_branch: &Branch) -> Result<()> {
        let branches = shared.collect_branches()?;
        let mut roots = Vec::with_capacity(branches.len());

        for branch in branches {
            let mut conn = shared.store.db().acquire().await?;

            match branch.open_root(&mut conn).await {
                Ok(dir) => roots.push(dir),
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
                Err(error) => return Err(error),
            }
        }

        JointDirectory::new(Some(local_branch.clone()), roots)
            .merge(shared.store.db())
            .await?;

        Ok(())
    }
}

/// Remove outdated branches and snapshots.
mod prune {
    use super::*;

    pub(super) async fn run(shared: &Shared) -> Result<()> {
        let mut conn = shared.store.db().acquire().await?;

        let local_id = shared.local_branch().map(|branch| *branch.id());
        let (uptodate, outdated) =
            utils::partition_branches(&mut conn, shared.collect_branches()?, local_id.as_ref())
                .await?;

        // Remove outdated branches
        for branch in outdated {
            // Never remove local branch
            if Some(branch.id()) == local_id.as_ref() {
                continue;
            }

            // Don't remove branches that are in use. We get notified when they stop being used
            // so we can try again.
            if branch.is_any_blob_open() {
                log::trace!("not removing outdated branch {:?} - in use", branch.id());
                continue;
            }

            log::trace!("removing outdated branch {:?}", branch.id());
            shared.store.index.remove_branch(branch.id());
            branch.data().destroy(&mut conn).await?;
        }

        // Remove outdated snapshots
        for branch in uptodate {
            branch.data().remove_old_snapshots(&mut conn).await?;
        }

        Ok(())
    }
}

/// Scan repository blocks to identify which ones are missing and which are no longer needed.
mod scan {
    use super::*;
    use crate::{
        blob::BlockIds,
        blob_id::BlobId,
        block::{self, BlockId},
        db,
        joint_directory::{JointEntryRef, MissingVersionStrategy},
    };
    use async_recursion::async_recursion;

    #[derive(Copy, Clone)]
    pub(super) enum Mode {
        // require missing blocks and collect unreachable blocks
        RequireAndCollect,
        // only collect unreachable blocks
        Collect,
    }

    pub(super) async fn run(shared: &Shared, mode: Mode) -> Result<()> {
        prepare_unreachable_blocks(shared).await?;
        traverse_root(shared, mode).await?;
        remove_unreachable_blocks(shared).await?;

        Ok(())
    }

    async fn traverse_root(shared: &Shared, mode: Mode) -> Result<()> {
        let branches = shared.collect_branches()?;

        let mut versions = Vec::with_capacity(branches.len());
        let mut entries = Vec::new();
        let mut result = Ok(());

        for branch in branches {
            entries.push(BlockIds::new(branch.clone(), BlobId::ROOT));

            if result.is_ok() {
                let mut conn = shared.store.db().acquire().await?;

                match branch.open_root(&mut conn).await {
                    Ok(dir) => versions.push(dir),
                    Err(Error::EntryNotFound) => {
                        // `EntryNotFound` here just means this is a newly created branch with no
                        // content yet. It is safe to ignore it.
                        continue;
                    }
                    Err(error) => {
                        // Remember the error but keep processing the remaining branches so that we
                        // find all the missing blocks.
                        result = Err(error);
                    }
                }
            }
        }

        for entry in entries {
            process_blocks(shared, mode, entry).await?;
        }

        // If there was en error opening any version of the root directory we can't proceed because
        // we might not have access to all the entries and we could fail to identify some missing
        // blocks and/or incorrectly mark some as unreachable.
        result?;

        let local_branch = shared.local_branch();
        traverse(shared, mode, JointDirectory::new(local_branch, versions)).await
    }

    #[async_recursion]
    async fn traverse(shared: &Shared, mode: Mode, dir: JointDirectory) -> Result<()> {
        let mut entries = Vec::new();
        let mut subdirs = Vec::new();
        let mut result = Ok(());

        let mut conn = shared.store.db().acquire().await?;

        // Collect the entries first, so we don't keep the directories locked while we are
        // processing the entries.
        for entry in dir.entries() {
            match entry {
                JointEntryRef::File(entry) => {
                    entries.push(BlockIds::new(
                        entry.inner().branch().clone(),
                        *entry.inner().blob_id(),
                    ));
                }
                JointEntryRef::Directory(entry) => {
                    for version in entry.versions() {
                        entries.push(BlockIds::new(version.branch().clone(), *version.blob_id()));
                    }

                    if result.is_ok() {
                        match entry.open(&mut conn, MissingVersionStrategy::Fail).await {
                            Ok(dir) => subdirs.push(dir),
                            Err(error) => {
                                // Remember the error but keep processing the remaining entries so
                                // that we find all the missing blocks.
                                result = Err(error);
                            }
                        }
                    }
                }
            }
        }

        drop(conn);

        for entry in entries {
            process_blocks(shared, mode, entry).await?;
        }

        // If there was en error opening any of the subdirectories we can't proceed further because
        // we might not have access to all the entries.
        result?;

        for dir in subdirs {
            traverse(shared, mode, dir).await?;
        }

        Ok(())
    }

    async fn prepare_unreachable_blocks(shared: &Shared) -> Result<()> {
        let mut conn = shared.store.db().acquire().await?;
        block::mark_all_unreachable(&mut conn).await
    }

    async fn remove_unreachable_blocks(shared: &Shared) -> Result<()> {
        let mut conn = shared.store.db().acquire().await?;
        let count = block::remove_unreachable(&mut conn).await?;

        if count > 0 {
            log::debug!("unreachable blocks removed: {}", count);
        }

        Ok(())
    }

    async fn process_blocks(shared: &Shared, mode: Mode, mut block_ids: BlockIds) -> Result<()> {
        let mut conn = shared.store.db().acquire().await?;

        while let Some(block_id) = block_ids.next(&mut conn).await? {
            block::mark_reachable(&mut conn, &block_id).await?;

            if matches!(mode, Mode::RequireAndCollect) {
                require_missing_block(shared, &mut conn, block_id).await?;
            }
        }

        Ok(())
    }

    async fn require_missing_block(
        shared: &Shared,
        conn: &mut db::Connection,
        block_id: BlockId,
    ) -> Result<()> {
        // TODO: check whether the block is already required to avoid the potentially expensive db
        // lookup.
        if !block::exists(conn, &block_id).await? {
            shared.store.block_tracker.require(block_id);
        }

        Ok(())
    }
}
